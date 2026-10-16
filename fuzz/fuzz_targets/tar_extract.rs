#![no_main]

//! Arbitrary tar entries (traversal, absolute paths, symlinks, hardlinks, odd entry types)
//! through `NpmCache::ensure`: extraction must never panic, never create anything outside the
//! package directory, and never touch the empty sentinel directory next to the cache.
//!
//! Limit: an escape into the cache's own `tmp-*` staging directory is deleted before the call
//! returns, so only escapes that reach the cache root or above are observable.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use arbitrary::Arbitrary;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use libfuzzer_sys::fuzz_target;
use sha2::{Digest, Sha512};
use velyr_compat_npm::cache::NpmCache;

#[derive(Arbitrary, Debug)]
enum Seg {
    Package,
    Up,
    Cur,
    Empty,
    Name(u8),
}

#[derive(Arbitrary, Debug)]
struct ArchivePath {
    absolute: bool,
    segments: Vec<Seg>,
}

impl ArchivePath {
    fn render(&self) -> String {
        let parts: Vec<&str> = self
            .segments
            .iter()
            .take(8)
            .map(|seg| match seg {
                Seg::Package => "package",
                Seg::Up => "..",
                Seg::Cur => ".",
                Seg::Empty => "",
                Seg::Name(n) => ["a", "b", "c", "d.js"][usize::from(*n) % 4],
            })
            .collect();
        let joined = parts.join("/");
        if self.absolute {
            format!("/{joined}")
        } else {
            joined
        }
    }
}

#[derive(Arbitrary, Debug)]
enum Kind {
    Dir,
    File(Vec<u8>),
    Symlink,
    Hardlink,
    Other(u8),
}

#[derive(Arbitrary, Debug)]
struct Entry {
    path: ArchivePath,
    link: ArchivePath,
    kind: Kind,
}

fn build_tar(entries: &[Entry]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for entry in entries.iter().take(16) {
        let mut header = tar::Header::new_gnu();
        let (entry_type, data): (tar::EntryType, &[u8]) = match &entry.kind {
            Kind::Dir => (tar::EntryType::Directory, &[]),
            Kind::File(data) => (tar::EntryType::Regular, &data[..data.len().min(256)]),
            Kind::Symlink => (tar::EntryType::Symlink, &[]),
            Kind::Hardlink => (tar::EntryType::Link, &[]),
            Kind::Other(byte) => (tar::EntryType::new(*byte), &[]),
        };
        header.set_entry_type(entry_type);
        header.set_mode(0o644);
        header.set_size(data.len() as u64);
        // Raw field writes: `set_path` would reject the traversal we want to feed in.
        let old = header.as_old_mut();
        copy_field(&mut old.name, &entry.path.render());
        copy_field(&mut old.linkname, &entry.link.render());
        header.set_cksum();
        builder.append(&header, data).expect("append to memory");
    }
    builder.into_inner().expect("finish tar")
}

fn copy_field(field: &mut [u8], value: &str) {
    let len = value.len().min(field.len());
    field[..len].copy_from_slice(&value.as_bytes()[..len]);
}

fn gzip(raw: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(raw).expect("gzip to memory");
    encoder.finish().expect("finish gzip")
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
    })
}

fn names(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(dir)
        .expect("read dir")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    found.sort();
    found
}

/// Resolves `.` and `..` without touching the filesystem (for dangling symlinks).
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn assert_inside(root: &Path, dir: &Path) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        let meta = fs::symlink_metadata(&path).expect("metadata");
        let real = match fs::canonicalize(&path) {
            Ok(real) => real,
            Err(_) => {
                let target = fs::read_link(&path).expect("dangling entry must be a symlink");
                lexical(&path.parent().expect("parent").join(target))
            }
        };
        assert!(
            real.starts_with(root),
            "{path:?} resolves outside {root:?}: {real:?}"
        );
        if meta.is_dir() {
            assert_inside(root, &path);
        }
    }
}

fuzz_target!(|entries: Vec<Entry>| {
    let tarball = gzip(&build_tar(&entries));
    let integrity = format!("sha512-{}", STANDARD.encode(Sha512::digest(&tarball)));

    let base = tempfile::tempdir().expect("tempdir");
    let base_real = fs::canonicalize(base.path()).expect("canonical base");
    let cache_root = base_real.join("cache");
    fs::create_dir(&cache_root).expect("cache dir");
    fs::create_dir(base_real.join("sentinel")).expect("sentinel dir");

    let cache = NpmCache::at(&cache_root);
    let result = runtime().block_on(cache.ensure(&integrity, |dest| async move {
        fs::write(dest, &tarball)
            .map_err(|e| velyr_runtime_errors::VelyrError::internal("write").with_cause(e))
    }));

    assert_eq!(
        names(&base_real),
        ["cache", "sentinel"],
        "entry created beside the cache"
    );
    assert!(
        names(&base_real.join("sentinel")).is_empty(),
        "sentinel was written to"
    );
    match result {
        Ok(package) => {
            let package = fs::canonicalize(package).expect("package dir");
            assert_eq!(
                package.parent(),
                Some(cache_root.as_path()),
                "unexpected cache layout"
            );
            assert_eq!(names(&cache_root).len(), 1, "stray entry in the cache root");
            assert_inside(&package, &package);
        }
        Err(_) => assert!(
            names(&cache_root).is_empty(),
            "rejected tarball left residue"
        ),
    }
});

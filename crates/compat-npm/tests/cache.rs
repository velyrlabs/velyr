//! Global npm cache tests: content addressing, atomic writes and safe extraction.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha512};
use tar::{Builder, EntryType, Header};
use velyr_compat_npm::cache::{ExtractLimits, NpmCache};
use velyr_runtime_errors::{ErrorCode, VelyrError};

fn sri(bytes: &[u8]) -> String {
    let digest = Sha512::digest(bytes);
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

fn hex(bytes: &[u8]) -> String {
    Sha512::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Appends an entry with a raw path, bypassing the builder's `..` checks.
fn raw_entry(
    builder: &mut Builder<Vec<u8>>,
    path: &str,
    kind: EntryType,
    mode: u32,
    link: Option<&str>,
    data: &[u8],
) {
    let mut header = Header::new_gnu();
    header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
    if let Some(link) = link {
        header.as_old_mut().linkname[..link.len()].copy_from_slice(link.as_bytes());
    }
    header.set_entry_type(kind);
    header.set_mode(mode);
    header.set_size(data.len() as u64);
    header.set_cksum();
    builder.append(&header, data).unwrap();
}

fn file(builder: &mut Builder<Vec<u8>>, path: &str, data: &[u8]) {
    raw_entry(builder, path, EntryType::Regular, 0o644, None, data);
}

fn gzip(builder: Builder<Vec<u8>>) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&builder.into_inner().unwrap()).unwrap();
    encoder.finish().unwrap()
}

fn tarball(build: impl FnOnce(&mut Builder<Vec<u8>>)) -> Vec<u8> {
    let mut builder = Builder::new(Vec::new());
    build(&mut builder);
    gzip(builder)
}

fn plain_package() -> Vec<u8> {
    tarball(|b| file(b, "package/package.json", br#"{"name":"left-pad"}"#))
}

struct Fixture {
    dir: tempfile::TempDir,
    cache: NpmCache,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cache = NpmCache::at(dir.path().join("npm"));
        Self { dir, cache }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("npm")
    }

    async fn ensure(&self, bytes: &[u8]) -> Result<PathBuf, VelyrError> {
        self.ensure_with_integrity(&sri(bytes), bytes).await
    }

    async fn ensure_with_integrity(
        &self,
        integrity: &str,
        bytes: &[u8],
    ) -> Result<PathBuf, VelyrError> {
        let bytes = bytes.to_vec();
        self.cache
            .ensure(integrity, |dest| async move {
                tokio::fs::write(dest, bytes)
                    .await
                    .map_err(|e| VelyrError::internal("write failed").with_cause(e))
            })
            .await
    }
}

fn names_under(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            out.push(entry.file_name().to_string_lossy().into_owned());
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&entry.path(), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

#[tokio::test]
async fn reuses_entry_without_fetching_again() {
    let fx = Fixture::new();
    let bytes = plain_package();
    let calls = AtomicUsize::new(0);
    let mut dirs = Vec::new();
    for _ in 0..2 {
        let payload = bytes.clone();
        let dir = fx
            .cache
            .ensure(&sri(&bytes), |dest| {
                calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    tokio::fs::write(dest, payload)
                        .await
                        .map_err(|e| VelyrError::internal("write failed").with_cause(e))
                }
            })
            .await
            .unwrap();
        dirs.push(dir);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(dirs[0], dirs[1]);
    assert_eq!(dirs[0], fx.root().join(hex(&bytes)));
    assert_eq!(
        fs::read_to_string(dirs[0].join("package.json")).unwrap(),
        r#"{"name":"left-pad"}"#
    );
    assert!(dirs[0].join(".velyr-meta.json").is_file());
}

#[tokio::test]
async fn rejects_parent_traversal_and_writes_nothing_outside_cache() {
    for evil in ["../../evil", "package/../../evil"] {
        let fx = Fixture::new();
        let bytes = tarball(|b| {
            file(b, "package/package.json", b"{}");
            file(b, evil, b"pwned");
        });
        let err = fx.ensure(&bytes).await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidData, "{evil}");
        assert!(!names_under(fx.dir.path()).contains(&"evil".to_owned()));
        assert!(!fx.root().join(hex(&bytes)).exists());
    }
}

#[tokio::test]
async fn interrupted_extraction_leaves_no_partial_directory() {
    let fx = Fixture::new();
    let big = vec![b'x'; 256 * 1024];
    let full = tarball(|b| {
        file(b, "package/package.json", b"{}");
        file(b, "package/big.bin", &big);
    });
    let truncated = &full[..full.len() * 2 / 3];
    let err = fx.ensure(truncated).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
    assert!(!fx.root().join(hex(truncated)).exists());
    assert_eq!(fs::read_dir(fx.root()).unwrap().count(), 0, "leftover tmp");
}

async fn assert_rejected(bytes: Vec<u8>) -> Fixture {
    let fx = Fixture::new();
    let err = fx.ensure(&bytes).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData, "{err:?}");
    assert!(!fx.root().join(hex(&bytes)).exists());
    assert_eq!(fs::read_dir(fx.root()).unwrap().count(), 0, "leftover tmp");
    fx
}

#[tokio::test]
async fn rejects_absolute_paths() {
    let fx = assert_rejected(tarball(|b| file(b, "/tmp/velyr-evil-abs", b"x"))).await;
    assert!(!names_under(fx.dir.path()).contains(&"velyr-evil-abs".to_owned()));
    assert!(!Path::new("/tmp/velyr-evil-abs").exists());
}

#[tokio::test]
async fn rejects_symlink_pointing_outside() {
    for target in ["../../outside", "/etc/passwd", "../../../.."] {
        assert_rejected(tarball(|b| {
            raw_entry(
                b,
                "package/link",
                EntryType::Symlink,
                0o777,
                Some(target),
                b"",
            );
        }))
        .await;
    }
}

#[tokio::test]
async fn rejects_symlink_chain_that_escapes_through_another_link() {
    // `a/b` points at the package root, so `a/b/c -> ..` really points outside.
    assert_rejected(tarball(|b| {
        raw_entry(
            b,
            "package/a/b",
            EntryType::Symlink,
            0o777,
            Some("../"),
            b"",
        );
        raw_entry(
            b,
            "package/a/b/c",
            EntryType::Symlink,
            0o777,
            Some(".."),
            b"",
        );
    }))
    .await;
}

#[tokio::test]
async fn rejects_symlink_target_with_non_leading_parent_segment() {
    assert_rejected(tarball(|b| {
        raw_entry(
            b,
            "package/l",
            EntryType::Symlink,
            0o777,
            Some("a/../../x"),
            b"",
        );
    }))
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn accepts_symlink_inside_package() {
    let fx = Fixture::new();
    let bytes = tarball(|b| {
        file(b, "package/lib/real.js", b"ok");
        raw_entry(
            b,
            "package/lib/alias.js",
            EntryType::Symlink,
            0o777,
            Some("real.js"),
            b"",
        );
        raw_entry(
            b,
            "package/lib/sibling.js",
            EntryType::Symlink,
            0o777,
            Some("../lib/real.js"),
            b"",
        );
    });
    let dir = fx.ensure(&bytes).await.unwrap();
    assert_eq!(fs::read_to_string(dir.join("lib/alias.js")).unwrap(), "ok");
    assert_eq!(
        fs::read_to_string(dir.join("lib/sibling.js")).unwrap(),
        "ok"
    );
}

#[tokio::test]
async fn rejects_hardlink_outside_or_unextracted() {
    for target in [
        "../../etc/passwd",
        "/etc/passwd",
        "package/not-extracted-yet",
    ] {
        assert_rejected(tarball(|b| {
            raw_entry(b, "package/hl", EntryType::Link, 0o644, Some(target), b"");
        }))
        .await;
    }
}

#[tokio::test]
async fn accepts_hardlink_to_extracted_file() {
    let fx = Fixture::new();
    let bytes = tarball(|b| {
        file(b, "package/a.txt", b"same");
        raw_entry(
            b,
            "package/b.txt",
            EntryType::Link,
            0o644,
            Some("package/a.txt"),
            b"",
        );
    });
    let dir = fx.ensure(&bytes).await.unwrap();
    assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "same");
}

#[tokio::test]
async fn rejects_device_and_fifo_entries() {
    for kind in [EntryType::Char, EntryType::Block, EntryType::Fifo] {
        assert_rejected(tarball(|b| {
            raw_entry(b, "package/dev", kind, 0o644, None, b"")
        }))
        .await;
    }
}

#[tokio::test]
async fn rejects_entry_named_like_cache_metadata() {
    assert_rejected(tarball(|b| file(b, "package/.velyr-meta.json", b"{}"))).await;
}

#[cfg(unix)]
#[tokio::test]
async fn keeps_only_the_executable_bit() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new();
    let bytes = tarball(|b| {
        raw_entry(
            b,
            "package/bin/cli.js",
            EntryType::Regular,
            0o755,
            None,
            b"#!/usr/bin/env node",
        );
        raw_entry(
            b,
            "package/readme.md",
            EntryType::Regular,
            0o664,
            None,
            b"hi",
        );
        raw_entry(b, "package/suid", EntryType::Regular, 0o4755, None, b"x");
        raw_entry(
            b,
            "package/sgid-sticky",
            EntryType::Regular,
            0o7644,
            None,
            b"x",
        );
    });
    let dir = fx.ensure(&bytes).await.unwrap();
    let mode = |p: &str| fs::metadata(dir.join(p)).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode("bin/cli.js"), 0o755);
    assert_eq!(mode("readme.md"), 0o644);
    assert_eq!(mode("suid"), 0o755);
    assert_eq!(mode("sgid-sticky"), 0o644);
}

#[tokio::test]
async fn enforces_size_quota() {
    let fx = Fixture::new();
    let cache = NpmCache::at(fx.root()).with_limits(ExtractLimits {
        max_total_bytes: 1024,
        max_entries: 100,
    });
    let bytes = tarball(|b| file(b, "package/big.bin", &[0u8; 4096]));
    let payload = bytes.clone();
    let err = cache
        .ensure(&sri(&bytes), |dest| async move {
            tokio::fs::write(dest, payload).await.unwrap();
            Ok(())
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::QuotaExceeded);
    assert_eq!(fs::read_dir(fx.root()).unwrap().count(), 0);
}

#[tokio::test]
async fn enforces_entry_quota() {
    let fx = Fixture::new();
    let cache = NpmCache::at(fx.root()).with_limits(ExtractLimits {
        max_total_bytes: 1024,
        max_entries: 3,
    });
    let bytes = tarball(|b| {
        for i in 0..10 {
            file(b, &format!("package/f{i}"), b"x");
        }
    });
    let payload = bytes.clone();
    let err = cache
        .ensure(&sri(&bytes), |dest| async move {
            tokio::fs::write(dest, payload).await.unwrap();
            Ok(())
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::QuotaExceeded);
    assert_eq!(fs::read_dir(fx.root()).unwrap().count(), 0);
}

#[tokio::test]
async fn rejects_tarball_that_does_not_match_integrity() {
    let fx = Fixture::new();
    let wrong = sri(b"something else");
    let err = fx
        .ensure_with_integrity(&wrong, &plain_package())
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
    assert_eq!(fs::read_dir(fx.root()).unwrap().count(), 0);
}

#[tokio::test]
async fn rejects_integrity_without_sha512() {
    let fx = Fixture::new();
    let err = fx
        .ensure_with_integrity("sha1-AAAAAAAAAAAAAAAAAAAAAAAAAAA=", &plain_package())
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
}

#[tokio::test]
async fn replaces_entry_with_corrupt_metadata() {
    let fx = Fixture::new();
    let bytes = plain_package();
    let dir = fx.ensure(&bytes).await.unwrap();
    fs::write(dir.join(".velyr-meta.json"), "not json").unwrap();
    fs::write(dir.join("stale.txt"), "stale").unwrap();
    let dir = fx.ensure(&bytes).await.unwrap();
    assert!(!dir.join("stale.txt").exists());
    assert!(dir.join("package.json").is_file());
}

#[tokio::test]
async fn failing_fetch_leaves_cache_clean() {
    let fx = Fixture::new();
    let err = fx
        .cache
        .ensure(&sri(b"x"), |_| async {
            Err(VelyrError::internal("offline"))
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Internal);
    assert_eq!(fs::read_dir(fx.root()).unwrap().count(), 0);
}

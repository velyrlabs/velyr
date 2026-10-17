//! Per-process cache of what `require` resolution asks the filesystem: whether a path is a file,
//! a directory or missing, and the parsed `package.json` of a directory. Resolving a graph of
//! thousands of modules otherwise repeats the same `stat`/`open` calls (extension probing,
//! ancestor walks). Entries never expire. `velyr run --watch` restarts the whole child process on
//! change, so it starts with an empty cache; [`clear_fs_caches`] is for hosts that reload in place.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use serde_json::Value;

use crate::format::{Format, detect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Dir,
    Missing,
}

/// `(mtime, len)` of a file; `None` for files served from the virtual root, which never change.
type Stamp = Option<(SystemTime, u64)>;

/// One read of a source file and the format decided from that text.
pub(crate) struct Read {
    pub text: Arc<str>,
    pub format: Format,
    stamp: Stamp,
}

struct Manifest {
    value: Arc<Value>,
    stamp: Stamp,
}

#[derive(Default)]
pub(crate) struct FsCache {
    kinds: Mutex<HashMap<PathBuf, Kind>>,
    manifests: Mutex<HashMap<PathBuf, Option<Manifest>>>,
    /// Reads made by `op_cjs_resolve`, consumed once by `op_cjs_read`.
    pending: Mutex<HashMap<PathBuf, Arc<Read>>>,
    /// Format of files already handed to `op_cjs_read`: `require.cache` holds them, so later
    /// resolves neither read nor stash them again.
    served: Mutex<HashMap<PathBuf, Format>>,
    syscalls: AtomicUsize,
}

static GLOBAL: LazyLock<FsCache> = LazyLock::new(FsCache::default);

impl FsCache {
    pub(crate) fn kind(&self, path: &Path) -> Kind {
        if let Some(hit) = self.kinds.lock().unwrap().get(path) {
            return *hit;
        }
        self.syscalls.fetch_add(1, Ordering::Relaxed);
        let kind = match (velyr_runtime_vfs::covering(path), path.to_str()) {
            (Some(vfs), Some(key)) if vfs.exists(key) => Kind::File,
            (Some(vfs), Some(key)) if vfs.is_dir(key) => Kind::Dir,
            (Some(_), _) => Kind::Missing,
            (None, _) => match fs::metadata(path) {
                Ok(meta) if meta.is_dir() => Kind::Dir,
                Ok(meta) if meta.is_file() => Kind::File,
                _ => Kind::Missing,
            },
        };
        self.kinds.lock().unwrap().insert(path.to_owned(), kind);
        kind
    }

    /// Parsed `dir/package.json`; `None` when it is missing, unreadable or not JSON. A cached
    /// manifest is revalidated with one `metadata` call, so a rewritten one is observed.
    pub(crate) fn manifest(&self, dir: &Path) -> Option<Arc<Value>> {
        let path = dir.join("package.json");
        if let Some(hit) = self.manifests.lock().unwrap().get(dir) {
            match hit {
                None => return None,
                Some(m) if m.stamp == stamp_of(&path) => return Some(m.value.clone()),
                Some(_) => {}
            }
        }
        self.syscalls.fetch_add(1, Ordering::Relaxed);
        let stamp = stamp_of(&path);
        let parsed = read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .map(|value| Manifest {
                value: Arc::new(value),
                stamp,
            });
        let value = parsed.as_ref().map(|m| m.value.clone());
        self.manifests
            .lock()
            .unwrap()
            .insert(dir.to_owned(), parsed);
        value
    }

    /// Format of `path` for `op_cjs_resolve`. The text is read only when detection has to sniff
    /// it, and then kept for the `read` that follows instead of being read again.
    pub(crate) fn classify(&self, path: &Path) -> Format {
        if let Some(format) = self.served.lock().unwrap().get(path) {
            return *format;
        }
        let mut sniffed = None;
        let format = detect(path, || {
            let (text, stamp) = read_source(path).ok()?;
            sniffed = Some((text.clone(), stamp));
            Some(text)
        });
        if let Some((text, stamp)) = sniffed
            && format != Format::Esm
        {
            let read = Arc::new(Read {
                text,
                format,
                stamp,
            });
            self.pending.lock().unwrap().insert(path.to_owned(), read);
        }
        format
    }

    /// Source and format of `path`: the read `classify` left behind when the file is unchanged
    /// since, else one fresh read. The kept copy is dropped here.
    pub(crate) fn read(&self, path: &Path) -> io::Result<Arc<Read>> {
        let kept = self.pending.lock().unwrap().remove(path);
        let read = match kept {
            Some(read) if read.stamp == stamp_of(path) => read,
            _ => {
                let (text, stamp) = read_source(path)?;
                let format = detect(path, || Some(text.clone()));
                Arc::new(Read {
                    text,
                    format,
                    stamp,
                })
            }
        };
        self.served
            .lock()
            .unwrap()
            .insert(path.to_owned(), read.format);
        Ok(read)
    }

    pub(crate) fn clear(&self) {
        self.kinds.lock().unwrap().clear();
        self.manifests.lock().unwrap().clear();
        self.pending.lock().unwrap().clear();
        self.served.lock().unwrap().clear();
    }

    #[cfg(test)]
    fn syscalls(&self) -> usize {
        self.syscalls.load(Ordering::Relaxed)
    }
}

/// `fs::read_to_string`, except that a file under the compiled executable's virtual root is read
/// from its payload (and a missing one is "not found" without touching the disk).
pub(crate) fn read_to_string(path: &Path) -> io::Result<String> {
    let Some(vfs) = velyr_runtime_vfs::covering(path) else {
        return fs::read_to_string(path);
    };
    let bytes = path
        .to_str()
        .and_then(|key| vfs.read(key))
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    String::from_utf8(bytes.to_vec()).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn stamp_of(path: &Path) -> Stamp {
    if velyr_runtime_vfs::covering(path).is_some() {
        return None;
    }
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// One counted read of a source file, stamped before the read so a later change is never missed.
fn read_source(path: &Path) -> io::Result<(Arc<str>, Stamp)> {
    #[cfg(feature = "test-counters")]
    counters::record(path);
    let stamp = stamp_of(path);
    Ok((read_to_string(path)?.into(), stamp))
}

#[cfg(feature = "test-counters")]
mod counters {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{LazyLock, Mutex};

    static READS: LazyLock<Mutex<HashMap<PathBuf, usize>>> = LazyLock::new(Mutex::default);

    pub(super) fn record(path: &Path) {
        *READS.lock().unwrap().entry(path.to_owned()).or_default() += 1;
    }

    pub(super) fn count(path: &Path) -> usize {
        READS.lock().unwrap().get(path).copied().unwrap_or(0)
    }
}

/// How many times the runtime read the source of `path` (test builds only).
#[cfg(feature = "test-counters")]
pub fn debug_read_count(path: &Path) -> usize {
    counters::count(path)
}

pub(crate) fn global() -> &'static FsCache {
    &GLOBAL
}

/// Forgets every cached file kind and manifest, for hosts that reload modules in place.
pub fn clear_fs_caches() {
    GLOBAL.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_probes_hit_the_filesystem_once() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.js"), "").unwrap();
        fs::write(dir.path().join("package.json"), r#"{"type":"module"}"#).unwrap();
        let cache = FsCache::default();
        for _ in 0..3 {
            assert_eq!(cache.kind(&dir.path().join("a.js")), Kind::File);
            assert_eq!(cache.kind(&dir.path().join("a.ts")), Kind::Missing);
            assert_eq!(cache.kind(dir.path()), Kind::Dir);
            assert!(cache.manifest(dir.path()).is_some());
            assert!(cache.manifest(&dir.path().join("sub")).is_none());
        }
        assert_eq!(cache.syscalls(), 5);
    }

    #[test]
    fn clear_makes_new_files_visible() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("late.js");
        let cache = FsCache::default();
        assert_eq!(cache.kind(&file), Kind::Missing);
        fs::write(&file, "").unwrap();
        assert_eq!(cache.kind(&file), Kind::Missing);
        cache.clear();
        assert_eq!(cache.kind(&file), Kind::File);
    }

    #[test]
    fn rewritten_manifest_is_observed() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("package.json");
        fs::write(&manifest, r#"{"type":"module"}"#).unwrap();
        let cache = FsCache::default();
        assert_eq!(cache.manifest(dir.path()).unwrap()["type"], "module");
        fs::write(&manifest, r#"{"type":"commonjs"}"#).unwrap();
        assert_eq!(cache.manifest(dir.path()).unwrap()["type"], "commonjs");
    }

    #[test]
    fn unchanged_file_between_resolve_and_read_is_not_reread() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.js");
        fs::write(&file, "exports.a = 1;").unwrap();
        let cache = FsCache::default();
        assert_eq!(cache.classify(&file), Format::CommonJs);
        let read = cache.read(&file).unwrap();
        assert_eq!(&*read.text, "exports.a = 1;");
        assert_eq!(cache.pending.lock().unwrap().len(), 0);
    }

    #[test]
    fn file_rewritten_between_resolve_and_read_is_reread() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.js");
        fs::write(&file, "exports.a = 1;").unwrap();
        let cache = FsCache::default();
        cache.classify(&file);
        fs::write(&file, "exports.a = 22;").unwrap();
        assert_eq!(&*cache.read(&file).unwrap().text, "exports.a = 22;");
    }
}

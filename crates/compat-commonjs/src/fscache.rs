//! Per-process cache of what `require` resolution asks the filesystem: whether a path is a file,
//! a directory or missing, and the parsed `package.json` of a directory. Resolving a graph of
//! thousands of modules otherwise repeats the same `stat`/`open` calls (extension probing,
//! ancestor walks). Entries never expire. `velyr run --watch` restarts the whole child process on
//! change, so it starts with an empty cache; [`clear_fs_caches`] is for hosts that reload in place.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Dir,
    Missing,
}

#[derive(Default)]
pub(crate) struct FsCache {
    kinds: Mutex<HashMap<PathBuf, Kind>>,
    manifests: Mutex<HashMap<PathBuf, Option<Arc<Value>>>>,
    syscalls: AtomicUsize,
}

static GLOBAL: LazyLock<FsCache> = LazyLock::new(FsCache::default);

impl FsCache {
    pub(crate) fn kind(&self, path: &Path) -> Kind {
        if let Some(hit) = self.kinds.lock().unwrap().get(path) {
            return *hit;
        }
        self.syscalls.fetch_add(1, Ordering::Relaxed);
        let kind = match fs::metadata(path) {
            Ok(meta) if meta.is_dir() => Kind::Dir,
            Ok(meta) if meta.is_file() => Kind::File,
            _ => Kind::Missing,
        };
        self.kinds.lock().unwrap().insert(path.to_owned(), kind);
        kind
    }

    /// Parsed `dir/package.json`; `None` when it is missing, unreadable or not JSON.
    pub(crate) fn manifest(&self, dir: &Path) -> Option<Arc<Value>> {
        if let Some(hit) = self.manifests.lock().unwrap().get(dir) {
            return hit.clone();
        }
        self.syscalls.fetch_add(1, Ordering::Relaxed);
        let parsed = fs::read_to_string(dir.join("package.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .map(Arc::new);
        self.manifests
            .lock()
            .unwrap()
            .insert(dir.to_owned(), parsed.clone());
        parsed
    }

    pub(crate) fn clear(&self) {
        self.kinds.lock().unwrap().clear();
        self.manifests.lock().unwrap().clear();
    }

    #[cfg(test)]
    fn syscalls(&self) -> usize {
        self.syscalls.load(Ordering::Relaxed)
    }
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
}

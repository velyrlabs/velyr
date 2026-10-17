//! Per-epoch memo of canonical parent directories.
//!
//! Only successful directory canonicalizations are stored; errors and symlink leaves always go
//! to `fs::canonicalize`, so a file that appears or is retargeted later is observed. The cache
//! only answers for paths the resolver already decided to canonicalize, so it adds no new
//! existence oracle.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::{fs, mem};

#[derive(Debug, Default)]
pub struct RealpathCache {
    dirs: RwLock<HashMap<PathBuf, PathBuf>>,
}

impl RealpathCache {
    /// Same result as `fs::canonicalize(path)`: one `symlink_metadata` for the leaf plus a memoized parent.
    pub fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        let is_plain_leaf = fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_symlink());
        let (Some(parent), Some(name), true) = (path.parent(), path.file_name(), is_plain_leaf)
        else {
            return fs::canonicalize(path);
        };
        if parent.as_os_str().is_empty() {
            return fs::canonicalize(path);
        }
        Ok(self.canonical_dir(parent)?.join(name))
    }

    fn canonical_dir(&self, dir: &Path) -> io::Result<PathBuf> {
        if let Some(hit) = self.dirs.read().unwrap_or_else(|e| e.into_inner()).get(dir) {
            return Ok(hit.clone());
        }
        let real = fs::canonicalize(dir)?;
        self.dirs
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(dir.to_path_buf(), real.clone());
        Ok(real)
    }

    /// Epoch boundary: called by the loader before every graph load.
    pub fn clear(&self) {
        mem::take(&mut *self.dirs.write().unwrap_or_else(|e| e.into_inner()));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::ResolveContext;

    fn tree() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.js"), "").unwrap();
        fs::write(root.join("sub/b.js"), "").unwrap();
        (dir, root)
    }

    #[test]
    fn matches_fs_canonicalize_for_plain_file() {
        let (_d, root) = tree();
        let cache = RealpathCache::default();
        for p in [
            root.join("a.js"),
            root.join("sub/../a.js"),
            root.join("sub"),
        ] {
            assert_eq!(
                cache.canonicalize(&p).unwrap(),
                fs::canonicalize(&p).unwrap()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_file_resolves_like_canonicalize() {
        let (_d, root) = tree();
        std::os::unix::fs::symlink(root.join("a.js"), root.join("link.js")).unwrap();
        let cache = RealpathCache::default();
        let got = cache.canonicalize(&root.join("link.js")).unwrap();
        assert_eq!(got, root.join("a.js"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_parent_dir_resolves_like_canonicalize() {
        let (_d, root) = tree();
        std::os::unix::fs::symlink(root.join("sub"), root.join("alias")).unwrap();
        let p = root.join("alias/b.js");
        let cache = RealpathCache::default();
        assert_eq!(
            cache.canonicalize(&p).unwrap(),
            fs::canonicalize(&p).unwrap()
        );
        assert_eq!(cache.canonicalize(&p).unwrap(), root.join("sub/b.js"));
    }

    #[cfg(unix)]
    #[test]
    fn retargeted_symlink_after_clear_is_observed() {
        let (_d, root) = tree();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(root.join("other/b.js"), "").unwrap();
        let link = root.join("alias");
        std::os::unix::fs::symlink(root.join("sub"), &link).unwrap();
        let cache = RealpathCache::default();
        let p = link.join("b.js");
        assert_eq!(cache.canonicalize(&p).unwrap(), root.join("sub/b.js"));
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(root.join("other"), &link).unwrap();
        assert_eq!(cache.canonicalize(&p).unwrap(), root.join("sub/b.js"));
        cache.clear();
        assert_eq!(cache.canonicalize(&p).unwrap(), root.join("other/b.js"));
    }

    #[test]
    fn missing_file_error_is_not_cached() {
        let (_d, root) = tree();
        let cache = RealpathCache::default();
        let p = root.join("late.js");
        assert!(cache.canonicalize(&p).is_err());
        fs::write(&p, "").unwrap();
        assert_eq!(cache.canonicalize(&p).unwrap(), p);
    }

    #[test]
    fn parent_is_canonicalized_once() {
        let (_d, root) = tree();
        fs::write(root.join("sub/c.js"), "").unwrap();
        let cache = RealpathCache::default();
        cache.canonicalize(&root.join("sub/b.js")).unwrap();
        // c.js exists, but its parent is served from the memo after the directory is gone.
        let moved = root.join("sub_moved");
        fs::rename(root.join("sub"), &moved).unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/c.js"), "").unwrap();
        assert_eq!(cache.dirs.read().unwrap().len(), 1);
        assert_eq!(
            cache.canonicalize(&root.join("sub/c.js")).unwrap(),
            root.join("sub/c.js")
        );
        assert_eq!(cache.dirs.read().unwrap().len(), 1);
        cache.clear();
        assert!(cache.dirs.read().unwrap().is_empty());
    }

    #[test]
    fn resolve_context_clone_shares_the_cache() {
        let ctx = ResolveContext {
            realpaths: Some(Arc::new(RealpathCache::default())),
            ..Default::default()
        };
        let copy = ctx.clone();
        assert!(Arc::ptr_eq(
            ctx.realpaths.as_ref().unwrap(),
            copy.realpaths.as_ref().unwrap()
        ));
        assert!(!format!("{ctx:?}").is_empty());
    }
}

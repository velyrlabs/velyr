//! On-disk cache of transformed modules: one JSON file per key, written atomically.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use sha2::{Digest, Sha256};
use velyr_runtime_modules::Transformed;

/// `<platform cache dir>/velyr/transform`, or `None` when the platform gives no home directory.
pub fn platform_cache_dir() -> Option<PathBuf> {
    let env_path = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(target_os = "macos") {
        env_path("HOME")?.join("Library/Caches")
    } else if cfg!(windows) {
        env_path("LOCALAPPDATA")?
    } else {
        env_path("XDG_CACHE_HOME").or_else(|| env_path("HOME").map(|h| h.join(".cache")))?
    };
    Some(base.join("velyr").join("transform"))
}

/// sha256 over length-prefixed parts, as lowercase hex.
pub fn key(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    format!("{:x}", hasher.finalize())
}

fn entry(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.json"))
}

/// A missing, unreadable or corrupt entry is a miss, as is an untrusted directory or entry
/// (see `ensure_private_dir`; on unix an entry must be a regular file owned by the directory owner).
pub fn read(dir: &Path, key: &str) -> Option<Transformed> {
    ensure_private_dir_once(dir).ok()?;
    let path = entry(dir, key);
    #[cfg(unix)]
    if !unix_trusted_entry(dir, &path) {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    Some(Transformed {
        code: json.get("code")?.as_str()?.to_owned(),
        source_map: json.get("map").and_then(|m| m.as_str()).map(str::to_owned),
    })
}

/// Creates `path` (if needed) as a private cache directory and returns its canonical path.
///
/// Rejects a symlink, a directory not owned by the effective uid, and anything it cannot probe.
/// Ownership is checked by creating a probe file (it belongs to our effective uid) and comparing
/// its uid with the directory's, so no extra dependency is needed. On success the mode is forced
/// to 0700. Non-unix: `create_dir_all` only (no ownership model checked).
pub fn ensure_private_dir(path: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    unix_private_dir(path)?;
    path.canonicalize()
}

#[cfg(unix)]
fn unix_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let denied = |msg: &str| io::Error::new(io::ErrorKind::PermissionDenied, msg.to_owned());
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(denied("cache directory is a symlink"));
    }
    let probe = path.join(format!(".probe-{}", std::process::id()));
    let _ = fs::remove_file(&probe);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?;
    let owner_matches = fs::metadata(path)
        .and_then(|dir| fs::metadata(&probe).map(|file| dir.is_dir() && dir.uid() == file.uid()));
    fs::remove_file(&probe)?;
    if !owner_matches? {
        return Err(denied("cache directory is owned by another user"));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// A regular file (not a symlink) owned by the same user as the already-verified private `dir`.
#[cfg(unix)]
fn unix_trusted_entry(dir: &Path, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    match (fs::symlink_metadata(path), fs::metadata(dir)) {
        (Ok(file), Ok(dir)) => file.file_type().is_file() && file.uid() == dir.uid(),
        _ => false,
    }
}

/// `ensure_private_dir` runs once per directory per process; later calls are free.
fn ensure_private_dir_once(dir: &Path) -> io::Result<()> {
    static CHECKED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let checked = CHECKED.get_or_init(Mutex::default);
    if checked.lock().is_ok_and(|set| set.contains(dir)) {
        return Ok(());
    }
    ensure_private_dir(dir)?;
    if let Ok(mut set) = checked.lock() {
        set.insert(dir.to_path_buf());
    }
    Ok(())
}

/// Creates `path` exclusively (mode 0600 on unix) and writes `bytes`.
fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Best effort: writes to a temp file in the same directory, then renames over the entry, so a
/// reader never sees a partial file. Failures only cost a future miss.
pub fn write(dir: &Path, key: &str, value: &Transformed) {
    let json = serde_json::json!({ "code": value.code, "map": value.source_map });
    let temp = dir.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let written = ensure_private_dir_once(dir)
        .and_then(|()| write_private_file(&temp, json.to_string().as_bytes()))
        .and_then(|()| fs::rename(&temp, entry(dir, key)));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn private_dir_is_0700() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a/b");
        let canonical = ensure_private_dir(&dir).unwrap();
        assert_eq!(canonical, dir.canonicalize().unwrap());
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn private_dir_tightens_loose_mode_of_own_dir() {
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o777)).unwrap();
        ensure_private_dir(tmp.path()).unwrap();
        assert_eq!(mode(tmp.path()), 0o700);
    }

    #[test]
    fn private_dir_rejects_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = ensure_private_dir(&link).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn private_dir_rejects_dir_owned_by_another_user() {
        let tmp = tempfile::tempdir().unwrap();
        if fs::metadata(tmp.path()).unwrap().uid() == 0 {
            return; // root owns everything it creates; nothing to reject
        }
        let before = mode(Path::new("/usr"));
        assert!(ensure_private_dir(Path::new("/usr")).is_err());
        assert_eq!(mode(Path::new("/usr")), before);
    }

    #[test]
    fn write_creates_private_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cache");
        let value = Transformed {
            code: "x".into(),
            source_map: None,
        };
        write(&dir, "k", &value);
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(read(&dir, "k").unwrap().code, "x");
    }

    fn sample() -> Transformed {
        Transformed {
            code: "x".into(),
            source_map: None,
        }
    }

    #[test]
    fn write_creates_entry_with_mode_0600() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "k", &sample());
        assert_eq!(mode(&entry(tmp.path(), "k")), 0o600);
    }

    #[test]
    fn read_refuses_symlink_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cache");
        write(&dir, "real", &sample());
        std::os::unix::fs::symlink(entry(&dir, "real"), entry(&dir, "k")).unwrap();
        assert!(read(&dir, "real").is_some());
        assert!(read(&dir, "k").is_none());
    }

    #[test]
    fn read_refuses_symlinked_cache_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        write(&real, "k", &sample());
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(read(&real, "k").is_some());
        assert!(read(&link, "k").is_none());
    }

    #[test]
    fn read_tightens_loose_cache_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cache");
        write(&dir, "k", &sample());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        let fresh = tmp.path().join("fresh");
        fs::create_dir(&fresh).unwrap();
        fs::copy(entry(&dir, "k"), entry(&fresh, "k")).unwrap();
        fs::set_permissions(&fresh, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(read(&fresh, "k").is_some());
        assert_eq!(mode(&fresh), 0o700);
    }
}

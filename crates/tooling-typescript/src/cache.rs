//! On-disk cache of transformed modules: one JSON file per key, written atomically.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// A missing, unreadable or corrupt entry is a miss.
pub fn read(dir: &Path, key: &str) -> Option<Transformed> {
    let json: serde_json::Value = serde_json::from_slice(&fs::read(entry(dir, key)).ok()?).ok()?;
    Some(Transformed {
        code: json.get("code")?.as_str()?.to_owned(),
        source_map: json.get("map").and_then(|m| m.as_str()).map(str::to_owned),
    })
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
    let written = fs::create_dir_all(dir)
        .and_then(|()| fs::write(&temp, json.to_string()))
        .and_then(|()| fs::rename(&temp, entry(dir, key)));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
}

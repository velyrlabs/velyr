//! `node_modules/.bin` entries from the `bin` field of top-level packages.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde_json::Value;

/// `bin name -> path relative to the package root`, sanitized: names are single path segments and
/// targets stay inside the package.
pub(super) fn declared(package_name: &str, manifest: &Value) -> BTreeMap<String, String> {
    let pairs: Vec<(String, String)> = match manifest.get("bin") {
        Some(Value::String(path)) => {
            let name = package_name.rsplit('/').next().unwrap_or(package_name);
            vec![(name.to_owned(), path.clone())]
        }
        Some(Value::Object(map)) => map
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
            .collect(),
        _ => Vec::new(),
    };
    pairs
        .into_iter()
        .filter(|(name, target)| safe_name(name) && safe_target(target))
        .map(|(name, target)| (name, target.trim_start_matches("./").to_owned()))
        .collect()
}

fn safe_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

fn safe_target(target: &str) -> bool {
    !target.is_empty()
        && Path::new(target)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(unix)]
pub(super) fn create(bin_dir: &Path, name: &str, link_target: &str) -> std::io::Result<()> {
    use std::os::unix::fs::symlink;
    std::fs::create_dir_all(bin_dir)?;
    let at = bin_dir.join(name);
    let _ = std::fs::remove_file(&at);
    symlink(link_target, &at)?;
    ensure_executable(&at);
    Ok(())
}

/// Makes the file behind `link` executable without touching the global cache: the file in
/// `node_modules` is usually a hardlink of the cached one, so a `chmod` would change both.
/// A non-executable target is replaced by an executable copy. Best effort: a missing target
/// is left alone.
#[cfg(unix)]
pub(super) fn ensure_executable(link: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(link) else {
        return;
    };
    let mode = meta.permissions().mode();
    if mode & 0o111 == 0o111 {
        return;
    }
    let Ok(file) = dunce::canonicalize(link) else {
        return;
    };
    let copy = file.with_file_name(format!(".velyr-bin-{}", std::process::id()));
    let made = std::fs::copy(&file, &copy)
        .and_then(|_| {
            std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(mode | 0o111))
        })
        .and_then(|()| std::fs::rename(&copy, &file));
    if made.is_err() {
        let _ = std::fs::remove_file(&copy);
    }
}

#[cfg(windows)]
pub(super) fn ensure_executable(_link: &Path) {}

#[cfg(windows)]
pub(super) fn create(bin_dir: &Path, name: &str, link_target: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(bin_dir)?;
    let target = link_target.replace('/', "\\");
    let shim = format!("@ECHO off\r\nnode \"%~dp0\\{target}\" %*\r\n");
    std::fs::write(bin_dir.join(format!("{name}.cmd")), shim)
}

pub(super) fn remove(bin_dir: &Path, name: &str) {
    let _ = std::fs::remove_file(bin_dir.join(name));
    let _ = std::fs::remove_file(bin_dir.join(format!("{name}.cmd")));
}

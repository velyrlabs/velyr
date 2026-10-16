//! `require` resolution: relative and absolute paths with extension probing, directories
//! (`package.json` `main`, `index`), and `node_modules` packages with `exports` conditions.
//! The `exports` and `imports` matching is compat-npm's (`match_exports`, `match_imports`).

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use velyr_compat_npm::resolve::exports::{match_exports, match_imports};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::Result;

const EXTENSIONS: [&str; 3] = ["js", "json", "cjs"];

fn not_found(request: &str, from: &Path) -> VelyrError {
    VelyrError::not_found(format!(
        "Cannot find module '{request}' (required from {})",
        from.display()
    ))
}

/// Lexically removes `.` and `..` so equal files get one `require.cache` key.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn real(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or_else(|_| normalize(&path))
}

fn load_as_file(base: &Path) -> Option<PathBuf> {
    if base.is_file() {
        return Some(base.to_owned());
    }
    EXTENSIONS.iter().find_map(|ext| {
        let mut name = base.as_os_str().to_owned();
        name.push(format!(".{ext}"));
        let candidate = PathBuf::from(name);
        candidate.is_file().then_some(candidate)
    })
}

fn load_as_directory(dir: &Path) -> Option<PathBuf> {
    let main = fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|manifest| manifest.get("main")?.as_str().map(str::to_owned));
    if let Some(main) = main {
        let target = dir.join(main);
        if let Some(found) = load_as_file(&target).or_else(|| load_as_index(&target)) {
            return Some(found);
        }
    }
    load_as_index(dir)
}

fn load_as_index(dir: &Path) -> Option<PathBuf> {
    load_as_file(&dir.join("index"))
}

fn load_path(target: &Path) -> Option<PathBuf> {
    load_as_file(target).or_else(|| load_as_directory(target))
}

/// `@scope/name/rest` into (`@scope/name`, `./rest`); `.` when there is no subpath.
fn split_specifier(request: &str) -> Option<(&str, String)> {
    let scope_len = match request.strip_prefix('@') {
        Some(rest) => rest.find('/')? + 2,
        None => 0,
    };
    let end = request[scope_len..]
        .find('/')
        .map_or(request.len(), |slash| scope_len + slash);
    let (name, rest) = request.split_at(end);
    (name.len() > scope_len).then(|| (name, format!(".{rest}")))
}

fn resolve_package(request: &str, from_dir: &Path, conditions: &[String]) -> Option<PathBuf> {
    let (name, subpath) = split_specifier(request)?;
    for ancestor in from_dir.ancestors() {
        if ancestor.file_name().is_some_and(|n| n == "node_modules") {
            continue;
        }
        let package = ancestor.join("node_modules").join(name);
        if !package.is_dir() {
            continue;
        }
        let exports = fs::read_to_string(package.join("package.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|manifest| manifest.get("exports").cloned());
        return match exports {
            Some(exports) => {
                let file = match_exports(&exports, &subpath, conditions, &package).ok()??;
                file.is_file().then_some(file)
            }
            None if subpath == "." => load_as_directory(&package),
            None => load_path(&package.join(subpath)),
        };
    }
    None
}

/// `#name` through the `imports` of the nearest `package.json`.
fn resolve_imports(request: &str, from_dir: &Path, conditions: &[String]) -> Option<PathBuf> {
    let (dir, manifest) = from_dir.ancestors().find_map(|dir| {
        let raw = fs::read_to_string(dir.join("package.json")).ok()?;
        Some((dir, serde_json::from_str::<Value>(&raw).ok()?))
    })?;
    let imports = manifest.get("imports")?;
    let external = |target: &str| {
        resolve_package(target, dir, conditions).ok_or_else(|| not_found(target, dir))
    };
    let file = match_imports(imports, request, conditions, dir, &external).ok()??;
    file.is_file().then_some(file)
}

/// Canonical path `require(request)` loads from the file `from`.
pub fn resolve(request: &str, from: &Path, conditions: &[String]) -> Result<PathBuf> {
    let from_dir = from.parent().unwrap_or(from);
    let found = if let Some(url) = request.strip_prefix("file://") {
        load_path(Path::new(url))
    } else if Path::new(request).is_absolute() {
        load_path(Path::new(request))
    } else if request == "."
        || request == ".."
        || request.starts_with("./")
        || request.starts_with("../")
    {
        load_path(&normalize(&from_dir.join(request)))
    } else if request.starts_with('#') {
        resolve_imports(request, from_dir, conditions)
    } else {
        resolve_package(request, from_dir, conditions)
    };
    found.map(real).ok_or_else(|| not_found(request, from))
}

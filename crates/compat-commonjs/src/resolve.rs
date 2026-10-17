//! `require` resolution: relative and absolute paths with extension probing, directories
//! (`package.json` `main`, `index`), and `node_modules` packages with `exports` conditions.
//! The `exports` and `imports` matching is compat-npm's (`match_exports`, `match_imports`).

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::fscache::{self, Auth, Kind};
use velyr_compat_npm::resolve::exports::{match_exports, match_imports};
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_modules::{MediaType, Result};

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

fn is_file(path: &Path) -> bool {
    fscache::global().kind(path) == Kind::File
}

fn is_dir(path: &Path) -> bool {
    fscache::global().kind(path) == Kind::Dir
}

fn load_as_file(base: &Path) -> Option<PathBuf> {
    if is_file(base) {
        return Some(base.to_owned());
    }
    EXTENSIONS.iter().find_map(|ext| {
        let mut name = base.as_os_str().to_owned();
        name.push(format!(".{ext}"));
        let candidate = PathBuf::from(name);
        is_file(&candidate).then_some(candidate)
    })
}

/// TypeScript sources a JavaScript specifier may stand for: `./x.cjs` is `x.cts`, `./x.js` is
/// `x.ts`, and `./x` may be either. Only tried when the requiring file is TypeScript itself.
fn load_typescript(base: &Path, auth: Auth) -> Option<PathBuf> {
    let name = base.file_name()?.to_str()?;
    let swapped = [(".cjs", "cts"), (".js", "ts"), (".mjs", "mts")]
        .iter()
        .find_map(|(js, ts)| name.strip_suffix(js).map(|stem| (stem, *ts)));
    let candidates: Vec<PathBuf> = match swapped {
        Some((stem, ts)) => vec![base.with_file_name(format!("{stem}.{ts}"))],
        None => ["ts", "cts"]
            .iter()
            .map(|ts| base.with_file_name(format!("{name}.{ts}")))
            .collect(),
    };
    // An unauthorized candidate reads as missing: probing it would reveal that it exists.
    candidates
        .into_iter()
        .find(|candidate| auth(candidate).is_ok() && is_file(candidate))
}

fn is_typescript(path: &Path) -> bool {
    MediaType::from_path(path) != MediaType::JavaScript
}

fn load_as_directory(dir: &Path, auth: Auth) -> Result<Option<PathBuf>> {
    let main = fscache::global()
        .manifest(dir, auth)?
        .and_then(|manifest| manifest.main.clone());
    if let Some(main) = main {
        let target = dir.join(main);
        if let Some(found) = load_as_file(&target).or_else(|| load_as_index(&target)) {
            return Ok(Some(found));
        }
    }
    Ok(load_as_index(dir))
}

fn load_as_index(dir: &Path) -> Option<PathBuf> {
    load_as_file(&dir.join("index"))
}

fn load_path(target: &Path, auth: Auth) -> Result<Option<PathBuf>> {
    match load_as_file(target) {
        Some(file) => Ok(Some(file)),
        None => load_as_directory(target, auth),
    }
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

fn resolve_package(
    request: &str,
    from_dir: &Path,
    conditions: &[String],
    auth: Auth,
) -> Result<Option<PathBuf>> {
    let Some((name, subpath)) = split_specifier(request) else {
        return Ok(None);
    };
    for ancestor in from_dir.ancestors() {
        if ancestor.file_name().is_some_and(|n| n == "node_modules") {
            continue;
        }
        let package = ancestor.join("node_modules").join(name);
        if !is_dir(&package) {
            continue;
        }
        let exports = fscache::global()
            .manifest(&package, auth)?
            .and_then(|manifest| manifest.exports.clone());
        return match exports {
            Some(exports) => {
                let file = match_exports(&exports, &subpath, conditions, &package)
                    .ok()
                    .flatten();
                Ok(file.filter(|file| is_file(file)))
            }
            None if subpath == "." => load_as_directory(&package, auth),
            None => load_path(&package.join(subpath), auth),
        };
    }
    Ok(None)
}

/// `#name` through the `imports` of the nearest `package.json`.
fn resolve_imports(
    request: &str,
    from_dir: &Path,
    conditions: &[String],
    auth: Auth,
) -> Result<Option<PathBuf>> {
    let Some((dir, manifest)) = fscache::global().nearest_manifest(from_dir, auth) else {
        return Ok(None);
    };
    let Some(imports) = manifest.imports.as_ref() else {
        return Ok(None);
    };
    let external = |target: &str| {
        resolve_package(target, dir, conditions, auth)?.ok_or_else(|| not_found(target, dir))
    };
    let file = match match_imports(imports, request, conditions, dir, &external) {
        Ok(file) => file,
        Err(err) if err.code() == ErrorCode::PermissionDenied => return Err(err),
        Err(_) => None,
    };
    Ok(file.filter(|file| is_file(file)))
}

/// Canonical path `require(request)` loads from the file `from`. `auth` gates every
/// `package.json` read; a denial surfaces as the error, except while looking for the nearest
/// manifest above a directory, which stops at the grant boundary.
pub fn resolve(request: &str, from: &Path, conditions: &[String], auth: Auth) -> Result<PathBuf> {
    let from_dir = from.parent().unwrap_or(from);
    let path_target = if let Some(url) = request.strip_prefix("file://") {
        Some(PathBuf::from(url))
    } else if Path::new(request).is_absolute() {
        Some(PathBuf::from(request))
    } else if request == "."
        || request == ".."
        || request.starts_with("./")
        || request.starts_with("../")
    {
        Some(normalize(&from_dir.join(request)))
    } else {
        None
    };
    let found = if let Some(target) = path_target {
        match load_path(&target, auth)? {
            None if is_typescript(from) => load_typescript(&target, auth),
            found => found,
        }
    } else if request.starts_with('#') {
        resolve_imports(request, from_dir, conditions, auth)?
    } else {
        resolve_package(request, from_dir, conditions, auth)?
    };
    found.map(real).ok_or_else(|| not_found(request, from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_candidates_are_authorized_before_they_are_probed() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::write(root.join("dep.ts"), "").unwrap();
        let from = root.join("main.cts");
        let deny_ts = |path: &Path| match path.extension().is_some_and(|ext| ext == "ts") {
            true => Err(VelyrError::permission_denied("import", "dep")),
            false => Ok(()),
        };
        let err = resolve("./dep", &from, &[], &deny_ts).unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound, "{err}");
        let allow = |_: &Path| Ok(());
        assert!(resolve("./dep", &from, &[], &allow).is_ok());
    }
}

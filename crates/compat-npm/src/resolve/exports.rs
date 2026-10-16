//! `exports` / `imports` target resolution (Node `PACKAGE_TARGET_RESOLVE`).

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_modules::Result;

/// Result of resolving one target. `Blocked` is an explicit `null`; `Undefined` means no
/// condition matched. Only the condition walk tells them apart: `Blocked` stops it.
pub(super) enum Outcome {
    Found(PathBuf),
    Blocked,
    Undefined,
}

/// Resolves a package specifier named by an `imports` target.
pub(super) type ExternalResolver<'a> = &'a dyn Fn(&str) -> Result<PathBuf>;

pub(super) struct TargetCx<'a> {
    pub conditions: &'a [String],
    pub package_dir: &'a Path,
    /// `Some` for `imports`, whose targets may name another package.
    pub external: Option<ExternalResolver<'a>>,
}

/// Exact key, else the `*` pattern with the longest prefix (then the longest key).
pub(super) fn match_key<'a>(
    map: &'a Map<String, Value>,
    key: &str,
) -> Option<(&'a Value, Option<String>)> {
    if !key.contains('*')
        && let Some(value) = map.get(key)
    {
        return Some((value, None));
    }
    map.iter()
        .filter_map(|(pattern, value)| {
            let (prefix, suffix) = pattern.split_once('*')?;
            let fits = !suffix.contains('*')
                && key.starts_with(prefix)
                && key.ends_with(suffix)
                && key.len() >= prefix.len() + suffix.len();
            fits.then(|| {
                let captured = key[prefix.len()..key.len() - suffix.len()].to_owned();
                ((prefix.len(), pattern.len()), value, captured)
            })
        })
        .max_by_key(|(rank, ..)| *rank)
        .map(|(_, value, captured)| (value, Some(captured)))
}

pub(super) fn resolve_target(
    target: &Value,
    pattern: Option<&str>,
    cx: &TargetCx,
) -> Result<Outcome> {
    match target {
        Value::String(text) => resolve_string(text, pattern, cx),
        Value::Array(items) => resolve_array(items, pattern, cx),
        Value::Object(map) => resolve_conditions(map, pattern, cx),
        Value::Null => Ok(Outcome::Blocked),
        _ => Err(invalid_target(target, cx)),
    }
}

fn resolve_string(text: &str, pattern: Option<&str>, cx: &TargetCx) -> Result<Outcome> {
    let substituted = match pattern {
        Some(captured) => text.replace('*', captured),
        None => text.to_owned(),
    };
    if let Some(rest) = text.strip_prefix("./") {
        let valid =
            !has_forbidden_segment(rest) && pattern.is_none_or(|p| !has_forbidden_segment(p));
        if !valid {
            return Err(invalid_target(&Value::from(text), cx));
        }
        return Ok(Outcome::Found(
            cx.package_dir.join(&substituted["./".len()..]),
        ));
    }
    match cx.external {
        Some(resolve_package) if !text.starts_with("../") && !text.starts_with('/') => {
            resolve_package(&substituted).map(Outcome::Found)
        }
        _ => Err(invalid_target(&Value::from(text), cx)),
    }
}

fn resolve_array(items: &[Value], pattern: Option<&str>, cx: &TargetCx) -> Result<Outcome> {
    for item in items {
        match resolve_target(item, pattern, cx) {
            Ok(found @ Outcome::Found(_)) => return Ok(found),
            Ok(_) => {}
            Err(err) if err.code() == ErrorCode::InvalidData => {}
            Err(err) => return Err(err),
        }
    }
    Ok(Outcome::Undefined)
}

fn resolve_conditions(
    map: &Map<String, Value>,
    pattern: Option<&str>,
    cx: &TargetCx,
) -> Result<Outcome> {
    for (condition, value) in map {
        if condition == "default" || cx.conditions.contains(condition) {
            match resolve_target(value, pattern, cx)? {
                Outcome::Undefined => {}
                decided => return Ok(decided),
            }
        }
    }
    Ok(Outcome::Undefined)
}

/// shortcut: percent-encoded dots are folded, other encodings are not; upgrade with a URL parser.
fn has_forbidden_segment(path: &str) -> bool {
    path.split(['/', '\\']).any(|segment| {
        let folded = segment.to_ascii_lowercase().replace("%2e", ".");
        matches!(folded.as_str(), "." | ".." | "node_modules")
    })
}

fn invalid_target(target: &Value, cx: &TargetCx) -> VelyrError {
    VelyrError::invalid_data(format!(
        "invalid package target {target} defined in {}",
        cx.package_dir.join("package.json").display()
    ))
}

/// `exports` sugar: a string, array or condition object stands for `{".": value}`.
fn normalize_exports<'a>(
    exports: &'a Value,
    package_dir: &Path,
) -> Result<Cow<'a, Map<String, Value>>> {
    let sugar = || Cow::Owned(Map::from_iter([(".".to_owned(), exports.clone())]));
    let Value::Object(map) = exports else {
        return Ok(sugar());
    };
    let dotted = map.keys().filter(|key| key.starts_with('.')).count();
    match dotted {
        0 => Ok(sugar()),
        n if n == map.len() => Ok(Cow::Borrowed(map)),
        _ => Err(VelyrError::invalid_data(format!(
            "invalid package config {}: \"exports\" mixes subpath and condition keys",
            package_dir.join("package.json").display()
        ))),
    }
}

/// Maps `subpath` (`.` or `./x`) through a package's `exports` for `conditions`. `None` when the
/// package does not export it (no key, no matching condition, or an explicit `null`); `Some` is
/// the target path, not checked for existence.
pub fn match_exports(
    exports: &Value,
    subpath: &str,
    conditions: &[String],
    package_dir: &Path,
) -> Result<Option<PathBuf>> {
    let map = normalize_exports(exports, package_dir)?;
    let Some((target, captured)) = match_key(&map, subpath) else {
        return Ok(None);
    };
    let cx = TargetCx {
        conditions,
        package_dir,
        external: None,
    };
    Ok(match resolve_target(target, captured.as_deref(), &cx)? {
        Outcome::Found(path) => Some(path),
        Outcome::Blocked | Outcome::Undefined => None,
    })
}

/// Maps a `#specifier` through a package's `imports`. Targets that name another package go
/// through `external`. `None` when the specifier is not defined.
pub fn match_imports(
    imports: &Value,
    specifier: &str,
    conditions: &[String],
    package_dir: &Path,
    external: &dyn Fn(&str) -> Result<PathBuf>,
) -> Result<Option<PathBuf>> {
    let Some((target, captured)) = imports
        .as_object()
        .and_then(|map| match_key(map, specifier))
    else {
        return Ok(None);
    };
    let cx = TargetCx {
        conditions,
        package_dir,
        external: Some(external),
    };
    Ok(match resolve_target(target, captured.as_deref(), &cx)? {
        Outcome::Found(path) => Some(path),
        Outcome::Blocked | Outcome::Undefined => None,
    })
}

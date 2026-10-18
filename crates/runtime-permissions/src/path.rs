//! Path policy: lexical normalization, realpath of the longest existing prefix, case folding.
//!
//! TOCTOU: a decision describes the filesystem at check time only. Host operations MUST open
//! the canonical path that was verified (not the caller's string) and must not re-resolve it;
//! a symlink swapped in between check and open is otherwise not covered by this module.
//!
//! Known limits: lexical `..` is applied before symlink resolution (`link/..` is not what the
//! OS would pick), and Unicode normalization (NFC/NFD) is not applied.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Unix,
    Windows,
}

const NATIVE: Flavor = if cfg!(windows) {
    Flavor::Windows
} else {
    Flavor::Unix
};

/// macOS (APFS/HFS+ default) and Windows volumes are case-insensitive.
const FOLD_CASE: bool = cfg!(any(windows, target_os = "macos"));

/// Folded root followed by folded components. Coverage is component-wise prefix, so
/// `/app/data2` is never covered by `/app/data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalizedPath(Vec<String>);

impl NormalizedPath {
    /// Covers every path; used as the fail-closed form of an unresolvable deny pattern.
    pub(crate) fn everything() -> Self {
        Self(Vec::new())
    }

    /// Last component (folded); `None` for a bare root.
    pub(crate) fn file_name(&self) -> Option<&str> {
        self.0.get(1..)?.last().map(String::as_str)
    }

    pub(crate) fn covers(&self, other: &Self) -> bool {
        other.0.starts_with(&self.0)
    }

    /// `None` (callers must fail closed) when the path is empty, non-UTF-8, malformed for the
    /// platform, relative without a base, or cannot be resolved safely.
    pub(crate) fn resolve(raw: &str, base: Option<&Path>) -> Option<Self> {
        let (root, parts) = split(NATIVE, raw)?;
        let (root, base_parts) = match root {
            Some(root) => (root, Vec::new()),
            None => {
                let (base_root, base_parts) = split(NATIVE, base?.to_str()?)?;
                (base_root?, base_parts)
            }
        };
        let mut lexical = PathBuf::from(&root);
        lexical.extend(clean(base_parts.into_iter().chain(parts)));
        let canonical = canonicalize_prefix(&lexical)?;
        let (root, parts) = split(NATIVE, canonical.to_str()?)?;
        let mut folded = vec![fold(&root?)];
        folded.extend(parts.iter().map(|part| fold(part)));
        Some(Self(folded))
    }
}

pub(crate) fn fold(text: &str) -> String {
    if FOLD_CASE {
        text.to_lowercase()
    } else {
        text.to_owned()
    }
}

/// Resolves `.` and `..` lexically; `..` at the root stays at the root.
fn clean(parts: impl Iterator<Item = String>) -> Vec<String> {
    let mut stack: Vec<String> = Vec::new();
    for part in parts {
        match part.as_str() {
            "." => {}
            ".." => {
                stack.pop();
            }
            _ => stack.push(part),
        }
    }
    stack
}

/// Longest existing prefix via realpath, plus the (lexically clean) non-existent suffix.
/// A suffix component that exists but cannot be resolved (dangling or looping symlink,
/// permission error) fails closed: writing through a dangling link would leave the area.
fn canonicalize_prefix(lexical: &Path) -> Option<PathBuf> {
    let mut existing = lexical.to_path_buf();
    let mut suffix: Vec<OsString> = Vec::new();
    loop {
        match dunce::canonicalize(&existing) {
            Ok(mut real) => {
                real.extend(suffix.iter().rev());
                return Some(real);
            }
            Err(_) if fs::symlink_metadata(&existing).is_ok() => return None,
            Err(_) => {
                suffix.push(existing.file_name()?.to_owned());
                existing.pop();
            }
        }
    }
}

/// Splits into an optional native-form root and raw components (`.`/`..` kept).
/// Outer `None` means rejected: empty, NUL, or (Windows) forms that cannot be normalized
/// without the OS (drive-relative, rooted without drive, device paths, ADS, trailing dot/space).
fn split(flavor: Flavor, raw: &str) -> Option<(Option<String>, Vec<String>)> {
    if raw.is_empty() || raw.contains('\0') {
        return None;
    }
    match flavor {
        Flavor::Unix => {
            let root = raw.starts_with('/').then(|| "/".to_owned());
            Some((root, components(raw, &['/'])))
        }
        Flavor::Windows => split_windows(raw),
    }
}

fn components(raw: &str, separators: &[char]) -> Vec<String> {
    raw.split(separators)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn strip_prefix_ignore_case<'a>(raw: &'a str, prefix: &str) -> Option<&'a str> {
    let head = raw.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &raw[prefix.len()..])
}

fn split_windows(raw: &str) -> Option<(Option<String>, Vec<String>)> {
    const SEPS: &[char] = &['\\', '/'];
    let unified = raw.replace('/', "\\");
    let (rest, unc) = if let Some(rest) = strip_prefix_ignore_case(&unified, r"\\?\UNC\") {
        (rest, true)
    } else if let Some(rest) = unified.strip_prefix(r"\\?\") {
        (rest, false)
    } else if unified.starts_with(r"\\.\") {
        return None;
    } else if let Some(rest) = unified.strip_prefix(r"\\") {
        (rest, true)
    } else {
        (unified.as_str(), false)
    };
    let (root, parts) = if unc {
        let mut parts = components(rest, SEPS).into_iter();
        let (server, share) = (parts.next()?, parts.next()?);
        (Some(format!(r"\\{server}\{share}\")), parts.collect())
    } else {
        let bytes = rest.as_bytes();
        let has_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        if has_drive {
            if bytes.get(2) != Some(&b'\\') {
                return None;
            }
            (
                Some(format!("{}:\\", &rest[..1])),
                components(&rest[2..], SEPS),
            )
        } else if rest.starts_with('\\') || unified.starts_with(r"\\?\") {
            return None;
        } else {
            (None, components(rest, SEPS))
        }
    };
    parts
        .iter()
        .all(|part| valid_windows_component(part))
        .then_some((root, parts))
}

fn valid_windows_component(part: &str) -> bool {
    part == "."
        || part == ".."
        || !(part.ends_with('.')
            || part.ends_with(' ')
            || part.contains([':', '<', '>', '"', '|', '?', '*']))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(raw: &str) -> Option<(Option<String>, Vec<String>)> {
        split(Flavor::Windows, raw)
    }

    fn parsed(root: &str, parts: &[&str]) -> Option<(Option<String>, Vec<String>)> {
        Some((
            Some(root.to_owned()),
            parts.iter().map(|p| (*p).to_owned()).collect(),
        ))
    }

    #[test]
    fn windows_drive_separators_and_verbatim() {
        assert_eq!(win(r"C:\App\Data"), parsed(r"C:\", &["App", "Data"]));
        assert_eq!(win("c:/app/data"), parsed(r"c:\", &["app", "data"]));
        assert_eq!(win(r"\\?\C:\app\x"), parsed(r"C:\", &["app", "x"]));
        assert_eq!(win(r"//?/c:/app"), parsed(r"c:\", &["app"]));
    }

    #[test]
    fn windows_unc_is_its_own_root() {
        assert_eq!(
            win(r"\\srv\share\a\b"),
            parsed(r"\\srv\share\", &["a", "b"])
        );
        assert_eq!(win(r"\\?\UNC\srv\share\a"), parsed(r"\\srv\share\", &["a"]));
        assert_eq!(win(r"\\?\unc\srv\share"), parsed(r"\\srv\share\", &[]));
        assert_eq!(win(r"\\srv"), None);
    }

    #[test]
    fn windows_ambiguous_forms_are_rejected() {
        for raw in [
            r"C:foo",
            r"C:",
            r"\foo",
            r"\\.\pipe\x",
            r"\\?\Volume{1}\x",
            r"C:\a\b.",
            r"C:\a\b ",
            r"C:\a\f:stream",
            r"C:\a\x*",
            "",
        ] {
            assert_eq!(win(raw), None, "{raw}");
        }
    }

    #[test]
    fn windows_relative_has_no_root() {
        assert_eq!(
            win(r"a\..\b"),
            Some((None, vec!["a".into(), "..".into(), "b".into()]))
        );
    }

    #[test]
    fn clean_resolves_dots_and_clamps_at_root() {
        let out = clean(
            ["a", ".", "b", "..", "..", "..", "c"]
                .map(String::from)
                .into_iter(),
        );
        assert_eq!(out, vec!["c".to_owned()]);
    }

    #[test]
    fn coverage_is_component_wise() {
        let at = |p: &str| NormalizedPath::resolve(p, None).unwrap();
        assert!(at("/app/data").covers(&at("/app/data/x")));
        assert!(!at("/app/data").covers(&at("/app/data2")));
        assert!(NormalizedPath::everything().covers(&at("/x")));
    }

    #[test]
    fn relative_without_base_is_unresolvable() {
        assert_eq!(NormalizedPath::resolve("a/b", None), None);
    }
}

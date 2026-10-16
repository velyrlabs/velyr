#![no_main]

//! Path normalization seen through the public API only (the normalizer is private).
//!
//! Grant: every path kind on `<root>/in`, deny on `<root>/in/secret`. Properties:
//! 1. `check` never panics and is deterministic.
//! 2. Allow implies the independently canonicalized, case-folded path lies under the grant and
//!    outside the deny (so no `..`, prefix or case trick escapes).
//! 3. Normalization is idempotent: a request and its lexically cleaned spelling decide alike.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use libfuzzer_sys::fuzz_target;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern, Request};

const FOLD_CASE: bool = cfg!(any(windows, target_os = "macos"));

struct Fixture {
    _dir: tempfile::TempDir,
    set: Arc<CapabilitySet>,
    root: String,
    allow_dir: String,
    allow_root: Vec<String>,
    deny_root: Vec<String>,
}

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonical root");
        let allow = root.join("in");
        let deny = allow.join("secret");
        fs::create_dir_all(&deny).expect("fixture dirs");
        fs::create_dir_all(root.join("out")).expect("fixture dirs");
        std::os::unix::fs::symlink(root.join("out"), allow.join("link")).expect("symlink");
        let pattern = |p: &Path| PathPattern::new(p.to_str().expect("utf-8 temp path"));
        let mut builder = CapabilitySet::builder();
        for make in [
            Capability::FsRead,
            Capability::FsWrite,
            Capability::ImportLocal,
            Capability::NativeLoad,
        ] {
            builder = builder
                .allow(make(pattern(&allow)))
                .deny(make(pattern(&deny)));
        }
        Fixture {
            root: root.to_str().expect("utf-8 temp path").to_owned(),
            allow_dir: allow.to_str().expect("utf-8 temp path").to_owned(),
            _dir: dir,
            set: builder.build(),
            allow_root: folded_components(&allow),
            deny_root: folded_components(&deny),
        }
    })
}

fn fold(part: &str) -> String {
    if FOLD_CASE {
        part.to_lowercase()
    } else {
        part.to_owned()
    }
}

fn folded_components(path: &Path) -> Vec<String> {
    path.to_str()
        .expect("utf-8 path")
        .split('/')
        .filter(|p| !p.is_empty())
        .map(fold)
        .collect()
}

/// Lexical `.`/`..` resolution of an absolute-or-cwd-relative path; `..` at the root is clamped.
fn clean(raw: &str, cwd: &Path) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let joined = if raw.starts_with('/') {
        raw.to_owned()
    } else {
        format!("{}/{raw}", cwd.to_str().expect("utf-8 cwd"))
    };
    for part in joined.split('/').filter(|p| !p.is_empty()) {
        match part {
            "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part.to_owned()),
        }
    }
    parts
}

/// realpath of the longest existing prefix plus the remaining suffix; `None` when an existing
/// component cannot be resolved.
fn canonical(parts: &[String]) -> Option<Vec<String>> {
    let mut existing = PathBuf::from("/");
    existing.extend(parts);
    let mut suffix: Vec<String> = Vec::new();
    loop {
        match fs::canonicalize(&existing) {
            Ok(real) => {
                let mut out = folded_components(&real);
                out.extend(suffix.iter().rev().map(|p| fold(p)));
                return Some(out);
            }
            Err(_) if fs::symlink_metadata(&existing).is_ok() => return None,
            Err(_) => {
                suffix.push(existing.file_name()?.to_str()?.to_owned());
                existing.pop();
            }
        }
    }
}

fn request(kind: u8, path: &str) -> Request<'_> {
    match kind % 4 {
        0 => Request::FsRead(path),
        1 => Request::FsWrite(path),
        2 => Request::ImportLocal(path),
        _ => Request::NativeLoad(path),
    }
}

fuzz_target!(|input: (u8, &str)| {
    let (selector, text) = input;
    let fx = fixture();
    // The temp root is unguessable, so also aim the input at the grant boundary.
    let prefixed = match selector >> 2 {
        1 => format!("{}/{text}", fx.root),
        2 => format!("{}/{text}", fx.allow_dir),
        _ => text.to_owned(),
    };
    let (kind, raw) = (selector, prefixed.as_str());
    let decision = fx.set.check(&request(kind, raw));
    assert_eq!(
        decision,
        fx.set.check(&request(kind, raw)),
        "non-deterministic: {raw:?}"
    );

    if raw.is_empty() || raw.contains('\0') {
        assert!(!decision.is_allow(), "empty/NUL path allowed: {raw:?}");
        return;
    }
    let cwd = std::env::current_dir().expect("cwd");
    let parts = clean(raw, &cwd);
    if decision.is_allow() {
        let real = canonical(&parts).unwrap_or_else(|| panic!("allowed unresolvable: {raw:?}"));
        assert!(
            real.starts_with(&fx.allow_root),
            "allowed outside grant: {raw:?} -> {real:?}"
        );
        assert!(
            !real.starts_with(&fx.deny_root),
            "allowed inside deny: {raw:?} -> {real:?}"
        );
    }

    let cleaned = format!("/{}", parts.join("/"));
    if !cleaned.contains('\0') {
        assert_eq!(
            decision.is_allow(),
            fx.set.check(&request(kind, &cleaned)).is_allow(),
            "normalization not idempotent: {raw:?} vs {cleaned:?}"
        );
    }
});

//! The only way to a spawnable program: resolve it, authorize it, and hand back the exact path to
//! execute. `Resolved` has no public constructor, and `spawn` accepts nothing else, so no code path
//! can start a process without passing through [`authorize_spawn`].
//!
//! Rules (`--allow-run`, `Capability::ProcessSpawn`):
//! - a bare name (no `/`) is looked up on the host `PATH`; it passes when the name or the resolved
//!   path is granted, and a deny on either wins;
//! - a name containing `/` is never an identity by itself: it is made absolute (against `cwd`),
//!   normalized, and passes only when that path (or its canonical form) is granted;
//! - the caller executes `Resolved::path`, which is the canonical path (symlinks resolved at check
//!   time), never the requested name, so the program that was authorized is the one that runs,
//!   whatever PATH the child is given or a symlink is later pointed at;
//! - the runtime's own executable is refused even when granted ([`GateError::RuntimeExecutable`]);
//! - a denial is decided before existence is looked at, so a missing and a forbidden program are
//!   indistinguishable to a caller that has no grant.

use std::ffi::OsStr;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use velyr_runtime_permissions::CapabilitySet;

/// An authorized, resolved program (its canonical path). Only [`authorize_spawn`] makes one.
#[derive(Debug, Clone)]
pub struct Resolved {
    path: PathBuf,
}

impl Resolved {
    /// Absolute, canonical path to execute.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateError {
    /// No grant covers the program (or it does not exist and no grant covers its name).
    Denied,
    /// Granted, but no executable was found.
    NotFound,
    /// Empty program, or a NUL byte in it.
    Invalid,
    /// The runtime's own executable: never spawnable, even when granted.
    RuntimeExecutable,
}

/// Lexical normalization: `.` and `..` are resolved without touching the file system, so a pattern
/// like `/usr/bin/*` cannot be escaped with `/usr/bin/../../tmp/x`.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// First executable named `name` in an absolute `PATH` entry. Empty and relative entries are
/// skipped: they would make the answer depend on the current directory.
fn find_in_path(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path_var?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| normalize(&dir.join(name)))
        .find(|candidate| is_executable_file(candidate))
}

/// Resolves `program` and checks it against `permissions`. `cwd` anchors a relative path with a
/// slash; `path_var` is the host's `PATH` (never one the script supplied).
pub fn authorize_spawn(
    permissions: &CapabilitySet,
    program: &str,
    cwd: &Path,
    path_var: Option<&OsStr>,
) -> Result<Resolved, GateError> {
    if program.is_empty() || program.contains('\0') {
        return Err(GateError::Invalid);
    }
    let candidate = if program.contains('/') {
        Some(normalize(&cwd.join(program)))
    } else {
        find_in_path(program, path_var)
    };
    let text = candidate.as_deref().and_then(Path::to_str);
    let canonical = candidate
        .as_ref()
        .and_then(|path| std::fs::canonicalize(path).ok());
    let canonical_text = canonical.as_deref().and_then(Path::to_str);
    let identities: Vec<&str> = text.into_iter().chain(canonical_text).collect();
    if !permissions.check_spawn(program, &identities).is_allow() {
        return Err(GateError::Denied);
    }
    // The canonical path is what runs, so a symlink re-pointed after this check cannot change the
    // program that was authorized.
    match canonical {
        Some(path) if is_executable_file(&path) => {
            if is_runtime_executable(&path) {
                return Err(GateError::RuntimeExecutable);
            }
            Ok(Resolved { path })
        }
        _ => Err(GateError::NotFound),
    }
}

/// True for the running runtime's own binary (same canonical path, or the same file through a hard
/// link). A child started from it would not inherit this process' permissions, and `velyr run -A`
/// would hand a script every permission, so it is refused whatever is granted, until permissions
/// can be inherited over a channel the script does not control.
fn is_runtime_executable(path: &Path) -> bool {
    let Some(own) = std::env::current_exe()
        .ok()
        .and_then(|exe| std::fs::canonicalize(exe).ok())
    else {
        return false;
    };
    if own == path {
        return true;
    }
    match (std::fs::metadata(&own), std::fs::metadata(path)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    }
}

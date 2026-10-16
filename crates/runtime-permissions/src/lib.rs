//! Capability model: immutable allow/deny sets and the ALLOW/DENY decision.
//!
//! Precedence: explicit deny > explicit scoped allow > default deny. There is no prompt.

mod derive;
mod net;
mod path;

pub use net::{AddrPattern, HostPattern};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use path::NormalizedPath;
use velyr_runtime_errors::{ErrorCode, VelyrError};

/// Exact text, or a prefix followed by a single trailing `*` (`AWS_*`, `*`).
fn matches_text(pattern: &str, text: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => text.starts_with(prefix),
        None => pattern == text,
    }
}

macro_rules! patterns {
    ($($name:ident),+) => {$(
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub fn new(pattern: impl Into<String>) -> Self {
                Self(pattern.into())
            }
        }
    )+};
}

patterns!(NamePattern, ProgramPattern);

/// Directory (or file) grant; a directory covers its whole subtree, never a string prefix.
/// Normalized and canonicalized when the set is built (see the `path` module).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPattern {
    raw: String,
    resolved: Option<NormalizedPath>,
}

impl PathPattern {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            raw: pattern.into(),
            resolved: None,
        }
    }

    /// `fallback` is used when the pattern cannot be resolved: nothing for allow, everything for deny.
    fn resolve(self, base: Option<&Path>, fallback: Option<NormalizedPath>) -> Self {
        let resolved = NormalizedPath::resolve(&self.raw, base).or(fallback);
        Self { resolved, ..self }
    }

    fn covers(&self, request: &NormalizedPath) -> bool {
        self.resolved.as_ref().is_some_and(|p| p.covers(request))
    }
}

/// Inclusive port range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    FsRead(PathPattern),
    FsWrite(PathPattern),
    NetConnect(HostPattern, PortRange),
    NetListen(AddrPattern, PortRange),
    EnvRead(NamePattern),
    EnvWrite(NamePattern),
    ProcessSpawn(ProgramPattern),
    ImportLocal(PathPattern),
    ImportRemote(HostPattern),
    NativeLoad(PathPattern),
    InspectorBind(AddrPattern),
}

/// A concrete operation to be checked against a [`CapabilitySet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    FsRead(&'a str),
    FsWrite(&'a str),
    NetConnect(&'a str, u16),
    NetListen(&'a str, u16),
    EnvRead(&'a str),
    EnvWrite(&'a str),
    ProcessSpawn(&'a str),
    ImportLocal(&'a str),
    ImportRemote(&'a str),
    NativeLoad(&'a str),
    InspectorBind(&'a str),
}

impl Request<'_> {
    /// Permission kind name; safe to print (the requested value is not).
    pub fn kind(&self) -> &'static str {
        match self {
            Request::FsRead(_) => "fs-read",
            Request::FsWrite(_) => "fs-write",
            Request::NetConnect(..) => "net-connect",
            Request::NetListen(..) => "net-listen",
            Request::EnvRead(_) => "env-read",
            Request::EnvWrite(_) => "env-write",
            Request::ProcessSpawn(_) => "process-spawn",
            Request::ImportLocal(_) => "import-local",
            Request::ImportRemote(_) => "import-remote",
            Request::NativeLoad(_) => "native-load",
            Request::InspectorBind(_) => "inspector-bind",
        }
    }
}

impl Capability {
    fn resolve_paths(self, base: Option<&Path>, fallback: Option<NormalizedPath>) -> Self {
        match self {
            Capability::FsRead(p) => Capability::FsRead(p.resolve(base, fallback)),
            Capability::FsWrite(p) => Capability::FsWrite(p.resolve(base, fallback)),
            Capability::ImportLocal(p) => Capability::ImportLocal(p.resolve(base, fallback)),
            Capability::NativeLoad(p) => Capability::NativeLoad(p.resolve(base, fallback)),
            other => other,
        }
    }

    /// `path` is the normalized request path for path-based kinds.
    fn covers(&self, request: &Request<'_>, path: Option<&NormalizedPath>) -> bool {
        match (self, request, path) {
            (Capability::EnvRead(p), Request::EnvRead(v), _) => matches_text(&p.0, v),
            (Capability::EnvWrite(p), Request::EnvWrite(v), _) => matches_text(&p.0, v),
            (Capability::ProcessSpawn(p), Request::ProcessSpawn(v), _) => matches_text(&p.0, v),
            (Capability::FsRead(p), Request::FsRead(_), Some(q))
            | (Capability::FsWrite(p), Request::FsWrite(_), Some(q))
            | (Capability::ImportLocal(p), Request::ImportLocal(_), Some(q))
            | (Capability::NativeLoad(p), Request::NativeLoad(_), Some(q)) => p.covers(q),
            // Network kinds are decided in `net` before this point.
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// `description` names the kind only, never the requested value.
    Deny {
        code: ErrorCode,
        description: String,
    },
}

impl Decision {
    fn denied(kind: &str) -> Self {
        Decision::Deny {
            code: ErrorCode::PermissionDenied,
            description: format!("{kind} access denied"),
        }
    }

    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }

    /// `Ok(())` on allow, a `VelyrError` with `PermissionDenied` on deny.
    pub fn into_result(self) -> Result<(), VelyrError> {
        match self {
            Decision::Allow => Ok(()),
            Decision::Deny { code, description } => Err(VelyrError::new(code, description)),
        }
    }
}

/// Immutable after [`CapabilitySetBuilder::build`]; cheap to share via `Arc`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilitySet {
    allow: Vec<Capability>,
    deny: Vec<Capability>,
    /// Process cwd at build time; relative paths (patterns and requests) resolve against it.
    base: Option<PathBuf>,
    /// Non-empty only for derived sets: every bound must allow (see the `derive` module).
    bounds: Vec<Arc<CapabilitySet>>,
}

impl CapabilitySet {
    pub fn builder() -> CapabilitySetBuilder {
        CapabilitySetBuilder::default()
    }

    pub fn check(&self, request: &Request<'_>) -> Decision {
        let denied = Decision::denied(request.kind());
        if !self.bounds.is_empty() {
            let allowed = self.bounds.iter().all(|b| b.check(request).is_allow());
            return if allowed { Decision::Allow } else { denied };
        }
        if let Some(allowed) = self.check_net(request) {
            return if allowed { Decision::Allow } else { denied };
        }
        let path = match request {
            Request::FsRead(p)
            | Request::FsWrite(p)
            | Request::ImportLocal(p)
            | Request::NativeLoad(p) => match NormalizedPath::resolve(p, self.base.as_deref()) {
                Some(path) => Some(path),
                None => return denied,
            },
            _ => None,
        };
        let covers = |c: &Capability| c.covers(request, path.as_ref());
        if !self.deny.iter().any(covers) && self.allow.iter().any(covers) {
            return Decision::Allow;
        }
        denied
    }
}

#[derive(Debug, Default)]
pub struct CapabilitySetBuilder {
    allow: Vec<Capability>,
    deny: Vec<Capability>,
}

impl CapabilitySetBuilder {
    pub fn allow(mut self, capability: Capability) -> Self {
        self.allow.push(capability);
        self
    }

    pub fn deny(mut self, capability: Capability) -> Self {
        self.deny.push(capability);
        self
    }

    pub fn build(self) -> Arc<CapabilitySet> {
        let base = std::env::current_dir().ok();
        let resolve_all = |caps: Vec<Capability>, fallback: Option<NormalizedPath>, deny: bool| {
            caps.into_iter()
                .map(|c| {
                    c.resolve_paths(base.as_deref(), fallback.clone())
                        .resolve_net(deny)
                })
                .collect()
        };
        Arc::new(CapabilitySet {
            allow: resolve_all(self.allow, None, false),
            deny: resolve_all(self.deny, Some(NormalizedPath::everything()), true),
            base,
            bounds: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(p: &str) -> NamePattern {
        NamePattern::new(p)
    }

    #[test]
    fn empty_set_denies_everything() {
        let set = CapabilitySet::builder().build();
        for request in [
            Request::EnvRead("HOME"),
            Request::FsRead("/etc/passwd"),
            Request::NetConnect("example.com", 443),
            Request::NativeLoad("/lib/x.so"),
        ] {
            assert!(!set.check(&request).is_allow(), "{request:?}");
        }
    }

    #[test]
    fn scoped_allow_matches_only_its_target() {
        let set = CapabilitySet::builder()
            .allow(Capability::EnvRead(env("DATABASE_URL")))
            .build();
        assert_eq!(
            set.check(&Request::EnvRead("DATABASE_URL")),
            Decision::Allow
        );
        assert!(!set.check(&Request::EnvRead("HOME")).is_allow());
        assert!(!set.check(&Request::EnvWrite("DATABASE_URL")).is_allow());
    }

    #[test]
    fn deny_beats_allow() {
        let set = CapabilitySet::builder()
            .allow(Capability::EnvRead(env("*")))
            .deny(Capability::EnvRead(env("SECRET_*")))
            .build();
        assert!(set.check(&Request::EnvRead("HOME")).is_allow());
        assert!(!set.check(&Request::EnvRead("SECRET_KEY")).is_allow());
    }

    #[test]
    fn other_kinds_match_their_own_patterns() {
        let set = CapabilitySet::builder()
            .allow(Capability::ProcessSpawn(ProgramPattern::new("git")))
            .allow(Capability::ImportRemote(HostPattern::new("esm.sh")))
            .allow(Capability::NativeLoad(PathPattern::new("/opt/lib.so")))
            .allow(Capability::InspectorBind(AddrPattern::new("127.0.0.1")))
            .build();
        assert!(set.check(&Request::ProcessSpawn("git")).is_allow());
        assert!(!set.check(&Request::ProcessSpawn("git2")).is_allow());
        assert!(set.check(&Request::ImportRemote("esm.sh")).is_allow());
        assert!(set.check(&Request::NativeLoad("/opt/lib.so")).is_allow());
        assert!(set.check(&Request::InspectorBind("127.0.0.1")).is_allow());
    }

    fn fs_read_set(pattern: &str) -> Arc<CapabilitySet> {
        CapabilitySet::builder()
            .allow(Capability::FsRead(PathPattern::new(pattern)))
            .build()
    }

    #[test]
    fn dotdot_and_string_prefix_do_not_escape_directory_grant() {
        let set = fs_read_set("/tmp/x");
        assert!(set.check(&Request::FsRead("/tmp/x")).is_allow());
        assert!(set.check(&Request::FsRead("/tmp/x/a/b")).is_allow());
        assert!(set.check(&Request::FsRead("/tmp/x/a/../b")).is_allow());
        assert!(!set.check(&Request::FsRead("/tmp/x/../y")).is_allow());
        assert!(!set.check(&Request::FsRead("/tmp/x2/a")).is_allow());
        assert!(
            !set.check(&Request::FsRead("/tmp/../../../etc/passwd"))
                .is_allow()
        );
    }

    #[test]
    fn read_grant_does_not_cover_write_or_import() {
        let set = fs_read_set("/tmp/x");
        assert!(!set.check(&Request::FsWrite("/tmp/x/a")).is_allow());
        assert!(!set.check(&Request::ImportLocal("/tmp/x/a")).is_allow());
    }

    #[test]
    fn relative_paths_resolve_against_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let set = fs_read_set("sub");
        let inside = cwd.join("sub/a");
        assert!(
            set.check(&Request::FsRead(inside.to_str().unwrap()))
                .is_allow()
        );
        assert!(set.check(&Request::FsRead("sub/a")).is_allow());
        assert!(!set.check(&Request::FsRead("sub/../other")).is_allow());
    }

    #[test]
    fn deny_pattern_is_normalized_too() {
        let set = CapabilitySet::builder()
            .allow(Capability::FsRead(PathPattern::new("/tmp")))
            .deny(Capability::FsRead(PathPattern::new("/tmp/secret/../s2")))
            .build();
        assert!(set.check(&Request::FsRead("/tmp/ok")).is_allow());
        assert!(!set.check(&Request::FsRead("/tmp/s2/k")).is_allow());
    }

    #[test]
    fn empty_and_nul_paths_are_denied() {
        let set = fs_read_set("/");
        assert!(!set.check(&Request::FsRead("")).is_allow());
        assert!(!set.check(&Request::FsRead("/a\0b")).is_allow());
    }

    #[test]
    fn native_load_uses_path_policy() {
        let set = CapabilitySet::builder()
            .allow(Capability::NativeLoad(PathPattern::new("/opt/lib")))
            .build();
        assert!(set.check(&Request::NativeLoad("/opt/lib/x.so")).is_allow());
        assert!(
            !set.check(&Request::NativeLoad("/opt/lib/../../etc/x"))
                .is_allow()
        );
        assert!(
            !set.check(&Request::NativeLoad("/opt/library/x.so"))
                .is_allow()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_out_of_allowed_area_is_denied() {
        let dir = tempfile::tempdir().unwrap();
        let allowed = dir.path().join("x");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("passwd"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, allowed.join("link")).unwrap();
        std::os::unix::fs::symlink(outside.join("missing"), allowed.join("dangling")).unwrap();
        std::os::unix::fs::symlink(&allowed, dir.path().join("alias")).unwrap();

        let set = fs_read_set(allowed.to_str().unwrap());
        let ask =
            |p: std::path::PathBuf| set.check(&Request::FsRead(p.to_str().unwrap())).is_allow();
        assert!(!ask(allowed.join("link/passwd")));
        assert!(!ask(allowed.join("link")));
        assert!(!ask(allowed.join("dangling")));
        assert!(!ask(allowed.join("dangling/new")));
        assert!(ask(allowed.join("new-file")));
        assert!(ask(dir.path().join("alias/new-file")));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_pattern_is_canonicalized() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("alias")).unwrap();
        let set = fs_read_set(dir.path().join("alias").to_str().unwrap());
        assert!(
            set.check(&Request::FsRead(real.join("f").to_str().unwrap()))
                .is_allow()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_matches_case_insensitively() {
        let set = fs_read_set("/Tmp/X");
        assert!(set.check(&Request::FsRead("/tmp/x/a")).is_allow());
    }

    #[cfg(windows)]
    #[test]
    fn windows_verbatim_and_case_match() {
        let set = fs_read_set(r"C:\App\Data");
        assert!(set.check(&Request::FsRead(r"\\?\c:\app\data\x")).is_allow());
        assert!(!set.check(&Request::FsRead(r"C:\app\data2\x")).is_allow());
    }

    #[test]
    fn deny_is_redacted_permission_denied() {
        let set = CapabilitySet::builder().build();
        let Decision::Deny { code, description } = set.check(&Request::EnvRead("SECRET_KEY"))
        else {
            panic!("expected deny");
        };
        assert_eq!(code, ErrorCode::PermissionDenied);
        assert!(description.contains("env-read"));
        assert!(!description.contains("SECRET_KEY"));
        let err = set.check(&Request::EnvRead("X")).into_result().unwrap_err();
        assert_eq!(err.code(), ErrorCode::PermissionDenied);
    }

    #[test]
    fn types_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Capability>();
        assert_send_sync::<CapabilitySet>();
        assert_send_sync::<Arc<CapabilitySet>>();
        assert_send_sync::<Decision>();
    }
}

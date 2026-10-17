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

patterns!(NamePattern);

/// Program name or path. As a deny rule it is matched by identity (see
/// [`CapabilitySet::check_spawn`]); a pattern with a `/` is canonicalized when the set is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramPattern {
    raw: String,
    resolved: Option<NormalizedPath>,
}

impl ProgramPattern {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            raw: pattern.into(),
            resolved: None,
        }
    }

    fn resolve(self, base: Option<&Path>, fallback: Option<NormalizedPath>) -> Self {
        if !self.raw.contains('/') {
            return self;
        }
        let resolved = NormalizedPath::resolve(&self.raw, base).or(fallback);
        Self { resolved, ..self }
    }

    /// Deny semantics: legacy text match on the raw identities, plus the resolved path for
    /// path patterns, plus case-folded name match against every basename for name patterns.
    fn denies(&self, spawn: &SpawnIdentity<'_>) -> bool {
        if spawn.raw.iter().any(|id| matches_text(&self.raw, id)) {
            return true;
        }
        match &self.resolved {
            Some(pattern) => spawn.canonical.iter().any(|p| pattern.covers(p)),
            None => {
                let pattern = path::fold(&self.raw);
                spawn.names.iter().any(|n| matches_text(&pattern, n))
            }
        }
    }
}

/// Every form a spawn request can be recognized by.
struct SpawnIdentity<'a> {
    /// Bare requested name and resolved paths, as given.
    raw: Vec<&'a str>,
    /// Resolved paths with symlinks followed.
    canonical: Vec<NormalizedPath>,
    /// Case-folded bare name and basenames of the given and canonical paths.
    names: Vec<String>,
}

impl<'a> SpawnIdentity<'a> {
    fn new(name: &'a str, paths: &[&'a str], base: Option<&Path>) -> Self {
        let bare = (!name.contains('/')).then_some(name);
        let canonical: Vec<NormalizedPath> = paths
            .iter()
            .filter_map(|p| NormalizedPath::resolve(p, base))
            .collect();
        let names = bare
            .into_iter()
            .chain(paths.iter().filter_map(|p| Path::new(p).file_name()?.to_str()))
            .map(path::fold)
            .chain(canonical.iter().filter_map(|p| p.file_name().map(str::to_owned)))
            .collect();
        Self {
            raw: bare.into_iter().chain(paths.iter().copied()).collect(),
            canonical,
            names,
        }
    }
}

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

    /// True when the pattern lies strictly below `root`.
    fn is_beneath(&self, root: &NormalizedPath) -> bool {
        self.resolved
            .as_ref()
            .is_some_and(|p| p != root && root.covers(p))
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
    /// System information by API name (`networkInterfaces`, `userInfo`, `loadavg`).
    SysInfo(NamePattern),
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
    SysInfo(&'a str),
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
            Request::SysInfo(_) => "sys-info",
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
            Capability::ProcessSpawn(p) => Capability::ProcessSpawn(p.resolve(base, fallback)),
            other => other,
        }
    }

    /// `path` is the normalized request path for path-based kinds.
    fn covers(&self, request: &Request<'_>, path: Option<&NormalizedPath>) -> bool {
        match (self, request, path) {
            (Capability::EnvRead(p), Request::EnvRead(v), _) => matches_text(&p.0, v),
            (Capability::EnvWrite(p), Request::EnvWrite(v), _) => matches_text(&p.0, v),
            (Capability::SysInfo(p), Request::SysInfo(v), _) => matches_text(&p.0, v),
            (Capability::ProcessSpawn(p), Request::ProcessSpawn(v), _) => matches_text(&p.raw, v),
            (Capability::FsRead(p), Request::FsRead(_), Some(q))
            | (Capability::FsWrite(p), Request::FsWrite(_), Some(q))
            | (Capability::ImportLocal(p), Request::ImportLocal(_), Some(q))
            | (Capability::NativeLoad(p), Request::NativeLoad(_), Some(q)) => p.covers(q),
            // Network kinds are decided in `net` before this point.
            _ => false,
        }
    }
}

impl Capability {
    /// A deny of the request's own kind that sits strictly below the request path.
    fn denies_beneath(&self, request: &Request<'_>, root: &NormalizedPath) -> bool {
        match (self, request) {
            (Capability::FsRead(p), Request::FsRead(_))
            | (Capability::FsWrite(p), Request::FsWrite(_)) => p.is_beneath(root),
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

    /// Whether a deny of the same kind lies strictly below the request path (`FsRead` and
    /// `FsWrite` only). Tree operations (`cp -r`, directory `rename`, `rm -r`) authorize only
    /// their root, so they use this to refuse when a nested deny would be read, moved or removed.
    /// Costs one pass over the deny list, never a walk of the tree. A path that cannot be
    /// resolved reports `true` (fail closed).
    pub fn has_deny_beneath(&self, request: &Request<'_>) -> bool {
        if !self.bounds.is_empty() {
            return self.bounds.iter().any(|b| b.has_deny_beneath(request));
        }
        let (Request::FsRead(raw) | Request::FsWrite(raw)) = request else {
            return false;
        };
        let Some(root) = NormalizedPath::resolve(raw, self.base.as_deref()) else {
            return true;
        };
        self.deny.iter().any(|c| c.denies_beneath(request, &root))
    }

    /// Process spawn by identity: `name` is the program as requested (only a bare name, one
    /// without `/`, counts as an identity) and `paths` are the absolute resolved paths of the
    /// executable. Any identity matching a deny rule denies (a deny also matches the basename of each
    /// path, case-insensitively on macOS, and compares canonicalized paths, so `deny sudo` covers
    /// `/usr/bin/sudo`, `SUDO` and a symlink to it); otherwise any identity matching an
    /// allow rule allows. So `allow-run=echo` lets `echo` run wherever it resolves, while
    /// `allow-run=/bin/echo` also covers it by path, and a deny on either form wins.
    pub fn check_spawn(&self, name: &str, paths: &[&str]) -> Decision {
        let denied = Decision::denied("process-spawn");
        if !self.bounds.is_empty() {
            let allowed = self.bounds.iter().all(|b| b.check_spawn(name, paths).is_allow());
            return if allowed { Decision::Allow } else { denied };
        }
        let spawn = SpawnIdentity::new(name, paths, self.base.as_deref());
        let denies = |c: &Capability| matches!(c, Capability::ProcessSpawn(p) if p.denies(&spawn));
        let allows = |c: &Capability| {
            spawn
                .raw
                .iter()
                .any(|id| c.covers(&Request::ProcessSpawn(id), None))
        };
        if !self.deny.iter().any(denies) && self.allow.iter().any(allows) {
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
    fn sys_info_is_scoped_by_name_and_separate_from_env() {
        let set = CapabilitySet::builder()
            .allow(Capability::SysInfo(env("loadavg")))
            .build();
        assert!(set.check(&Request::SysInfo("loadavg")).is_allow());
        assert!(!set.check(&Request::SysInfo("userInfo")).is_allow());
        assert!(!set.check(&Request::EnvRead("loadavg")).is_allow());
        let all = CapabilitySet::builder()
            .allow(Capability::SysInfo(env("*")))
            .deny(Capability::SysInfo(env("userInfo")))
            .build();
        assert!(all.check(&Request::SysInfo("networkInterfaces")).is_allow());
        assert!(!all.check(&Request::SysInfo("userInfo")).is_allow());
        assert_eq!(Request::SysInfo("x").kind(), "sys-info");
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

    fn run(pattern: &str) -> Arc<CapabilitySet> {
        CapabilitySet::builder()
            .allow(Capability::ProcessSpawn(ProgramPattern::new(pattern)))
            .build()
    }

    #[test]
    fn spawn_is_decided_by_bare_name_or_resolved_path() {
        let by_name = run("echo");
        assert!(by_name.check_spawn("echo", &["/bin/echo"]).is_allow());
        assert!(!by_name.check_spawn("ls", &["/bin/ls"]).is_allow());
        // A name with a slash is never an identity: only the resolved path counts.
        assert!(!by_name.check_spawn("./echo", &["/tmp/echo"]).is_allow());
        assert!(!by_name.check_spawn("/tmp/echo", &["/tmp/echo"]).is_allow());
        let by_path = run("/bin/echo");
        assert!(by_path.check_spawn("echo", &["/bin/echo"]).is_allow());
        assert!(by_path.check_spawn("/bin/echo", &["/bin/echo"]).is_allow());
        assert!(!by_path.check_spawn("echo", &["/tmp/echo"]).is_allow());
        assert!(!by_path.check_spawn("echo", &[]).is_allow());
    }

    #[test]
    fn spawn_deny_on_either_identity_wins() {
        let set = CapabilitySet::builder()
            .allow(Capability::ProcessSpawn(ProgramPattern::new("*")))
            .deny(Capability::ProcessSpawn(ProgramPattern::new("/bin/rm")))
            .deny(Capability::ProcessSpawn(ProgramPattern::new("sudo")))
            .build();
        assert!(set.check_spawn("echo", &["/bin/echo"]).is_allow());
        assert!(!set.check_spawn("rm", &["/bin/rm"]).is_allow());
        assert!(!set.check_spawn("sudo", &["/usr/bin/sudo"]).is_allow());
        assert!(!set.check_spawn("/usr/bin/sudo", &["/bin/rm"]).is_allow());
    }

    fn deny_spawn(allow: &str, deny: &str) -> Arc<CapabilitySet> {
        CapabilitySet::builder()
            .allow(Capability::ProcessSpawn(ProgramPattern::new(allow)))
            .deny(Capability::ProcessSpawn(ProgramPattern::new(deny)))
            .build()
    }

    #[test]
    fn deny_by_name_also_matches_basename_of_any_path_identity() {
        let set = deny_spawn("*", "sudo");
        assert!(!set.check_spawn("/usr/bin/sudo", &["/usr/bin/sudo"]).is_allow());
        assert!(!set.check_spawn("./sudo", &["/tmp/nowhere/sudo"]).is_allow());
        assert!(set.check_spawn("sudoku", &["/usr/bin/sudoku"]).is_allow());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn deny_by_name_ignores_case_on_macos() {
        let set = deny_spawn("*", "sudo");
        assert!(!set.check_spawn("SUDO", &["/usr/bin/SUDO"]).is_allow());
        assert!(!set.check_spawn("SuDo", &[]).is_allow());
    }

    #[cfg(unix)]
    #[test]
    fn deny_by_name_follows_symlink_to_denied_program() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("sudo");
        let link = dir.path().join("innocent");
        std::fs::write(&real, "x").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let set = deny_spawn("*", "sudo");
        let link = link.to_str().unwrap();
        assert!(!set.check_spawn("innocent", &[link]).is_allow());
    }

    #[cfg(unix)]
    #[test]
    fn deny_by_path_resolves_pattern_and_matches_through_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let usr = dir.path().join("usr/bin");
        std::fs::create_dir_all(&usr).unwrap();
        std::fs::write(usr.join("rm"), "x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("usr/bin"), dir.path().join("bin")).unwrap();
        let denied = dir.path().join("bin/rm");
        let set = deny_spawn("*", denied.to_str().unwrap());
        // Same file reached by the real path (usrmerge): the name resolves to it.
        let real = usr.join("rm");
        assert!(!set.check_spawn("rm", &[real.to_str().unwrap()]).is_allow());
        assert!(set.check_spawn("ls", &["/bin/ls"]).is_allow());
    }

    #[test]
    fn explicit_allowlist_is_unchanged() {
        let set = run("git");
        assert!(set.check_spawn("git", &["/usr/bin/git"]).is_allow());
        assert!(!set.check_spawn("/tmp/git", &["/tmp/git"]).is_allow());
        assert!(!set.check_spawn("sudo", &["/usr/bin/sudo"]).is_allow());
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
    fn deny_beneath_finds_nested_denies_of_the_same_kind_only() {
        let set = CapabilitySet::builder()
            .allow(Capability::FsWrite(PathPattern::new("/tmp/a")))
            .deny(Capability::FsWrite(PathPattern::new("/tmp/a/b/secret")))
            .build();
        let beneath = |request: Request<'_>| set.has_deny_beneath(&request);
        assert!(beneath(Request::FsWrite("/tmp/a")));
        assert!(beneath(Request::FsWrite("/tmp/a/b")));
        assert!(beneath(Request::FsWrite("/tmp/A/../a")));
        assert!(!beneath(Request::FsWrite("/tmp/a/b/secret/x")));
        assert!(!beneath(Request::FsWrite("/tmp/a/c")));
        assert!(!beneath(Request::FsWrite("/tmp/a2")));
        assert!(!beneath(Request::FsRead("/tmp/a")));
        assert!(!beneath(Request::EnvRead("HOME")));
    }

    #[test]
    fn deny_beneath_fails_closed_and_follows_bounds() {
        let denying = CapabilitySet::builder()
            .allow(Capability::FsRead(PathPattern::new("/tmp")))
            .deny(Capability::FsRead(PathPattern::new("/tmp/s")))
            .build();
        assert!(denying.has_deny_beneath(&Request::FsRead("")));
        let plain = CapabilitySet::builder()
            .allow(Capability::FsRead(PathPattern::new("/tmp")))
            .build();
        assert!(!plain.has_deny_beneath(&Request::FsRead("/tmp")));
        let child = plain.derive_child(&plain, &denying);
        assert!(child.has_deny_beneath(&Request::FsRead("/tmp")));
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

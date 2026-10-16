//! Permission flags and the `velyr.json` `permissions` block share one grammar (ADR-013).
//! [`build`] turns the effective grammar into the `CapabilitySet` and `QuotaConfig` for the runtime.

mod expand;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use serde::{Deserialize, Deserializer};
use velyr_runtime_permissions::{Capability, CapabilitySet};
use velyr_runtime_resources::QuotaConfig;

use crate::config::Loaded;
use expand::{Kind, path_pattern};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("error: cannot read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("error: invalid {}: {message}", path.display())]
    Parse { path: PathBuf, message: String },
    #[error("error: invalid value {value:?} for --{flag}: {reason}")]
    InvalidValue {
        flag: String,
        value: String,
        reason: &'static str,
    },
    #[error("error: cannot determine the working directory: {0}")]
    WorkingDir(std::io::Error),
}

/// One permission kind: `None` is absent, `Some([])` is the kind unrestricted (bare flag or
/// `true`), `Some(list)` is scoped. In the config, `false` and `[]` both mean absent.
type Scope = Option<Vec<String>>;

fn scope_from_config<'de, D: Deserializer<'de>>(d: D) -> Result<Scope, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Flag(bool),
        List(Vec<String>),
    }
    Ok(match Option::<Raw>::deserialize(d)? {
        Some(Raw::Flag(true)) => Some(Vec::new()),
        Some(Raw::List(list)) if !list.is_empty() => Some(list),
        _ => None,
    })
}

macro_rules! permissions {
    ($($field:ident: $help:literal),+ $(,)?) => {
        #[derive(Debug, Default, Clone, PartialEq, Eq, Args, Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "kebab-case")]
        pub struct Permissions {
            /// Grant everything except native loading and the inspector.
            #[arg(short = 'A', long)]
            #[serde(default)]
            pub allow_all: bool,
            $(
                #[arg(long, num_args = 0..=1, require_equals = true, value_delimiter = ',', value_name = $help)]
                #[serde(default, deserialize_with = "scope_from_config")]
                pub $field: Option<Vec<String>>,
            )+
        }
    };
}

permissions! {
    allow_read: "PATHS", allow_write: "PATHS", allow_net: "HOST[:PORT]",
    allow_listen: "ADDR[:PORT]", allow_env: "NAMES", allow_run: "PROGRAMS",
    allow_native: "PATHS", allow_import: "HOSTS", inspect: "ADDR",
    deny_read: "PATHS", deny_write: "PATHS", deny_net: "HOST[:PORT]",
    deny_listen: "ADDR[:PORT]", deny_env: "NAMES", deny_run: "PROGRAMS",
    deny_native: "PATHS", deny_import: "HOSTS", deny_inspect: "ADDR",
}

impl Permissions {
    fn allow_scopes(&self) -> [(Kind, &Scope); 9] {
        [
            (Kind::Read, &self.allow_read),
            (Kind::Write, &self.allow_write),
            (Kind::Net, &self.allow_net),
            (Kind::Listen, &self.allow_listen),
            (Kind::Env, &self.allow_env),
            (Kind::Run, &self.allow_run),
            (Kind::Native, &self.allow_native),
            (Kind::Import, &self.allow_import),
            (Kind::Inspect, &self.inspect),
        ]
    }

    fn deny_scopes(&self) -> [(Kind, &Scope); 9] {
        [
            (Kind::Read, &self.deny_read),
            (Kind::Write, &self.deny_write),
            (Kind::Net, &self.deny_net),
            (Kind::Listen, &self.deny_listen),
            (Kind::Env, &self.deny_env),
            (Kind::Run, &self.deny_run),
            (Kind::Native, &self.deny_native),
            (Kind::Import, &self.deny_import),
            (Kind::Inspect, &self.deny_inspect),
        ]
    }
}

/// Authority and limits handed to the runtime, plus warnings for the CLI to print.
#[derive(Debug)]
pub struct Built {
    pub capabilities: Arc<CapabilitySet>,
    pub quotas: QuotaConfig,
    pub warnings: Vec<String>,
}

const ALLOW_ALL_WARNING: &str =
    "warning: --allow-all grants every permission except native loading and the inspector";

/// Any permission flag replaces the config's `permissions` block; nothing is merged.
pub fn build(
    flags: Permissions,
    config: Option<&Loaded>,
    cwd: &Path,
    entry: &Path,
) -> Result<Built, ConfigError> {
    let from_config = config.and_then(|l| l.config.permissions.clone());
    let effective = if flags == Permissions::default() {
        from_config.unwrap_or_default()
    } else {
        flags
    };
    let root = match config {
        Some(loaded) => loaded.dir.clone(),
        None => entry_dir(cwd, entry),
    };
    // Compare the same form the module loader sees (symlinks resolved); keep the lexical path if it does not exist.
    let root = root.canonicalize().unwrap_or(root);

    let mut builder =
        CapabilitySet::builder().allow(Capability::ImportLocal(path_pattern(cwd, &root)));
    // The `node:` facades are generated files outside the project; nothing else is granted there.
    #[cfg(feature = "node")]
    if let Ok(dir) = crate::node::facade_dir() {
        builder = builder.allow(Capability::ImportLocal(path_pattern(cwd, dir)));
    }
    let mut warnings = Vec::new();
    let mut allow = Vec::new();
    if effective.allow_all {
        warnings.push(ALLOW_ALL_WARNING.to_owned());
        for kind in Kind::ALLOW_ALL {
            allow.extend(kind.capabilities(&[], cwd, false)?);
        }
    }
    allow.extend(collect(effective.allow_scopes(), cwd, false)?);
    for cap in allow {
        builder = builder.allow(cap);
    }
    for cap in collect(effective.deny_scopes(), cwd, true)? {
        builder = builder.deny(cap);
    }
    let quotas = config
        .and_then(|l| l.config.quotas.as_ref().map(|q| q.resources.clone()))
        .unwrap_or_default();
    Ok(Built {
        capabilities: builder.build(),
        quotas,
        warnings,
    })
}

fn collect(
    scopes: [(Kind, &Scope); 9],
    cwd: &Path,
    deny: bool,
) -> Result<Vec<Capability>, ConfigError> {
    let mut caps = Vec::new();
    for (kind, scope) in scopes {
        if let Some(scope) = scope {
            caps.extend(kind.capabilities(scope, cwd, deny)?);
        }
    }
    Ok(caps)
}

fn entry_dir(cwd: &Path, entry: &Path) -> PathBuf {
    cwd.join(entry)
        .parent()
        .map_or_else(|| cwd.to_path_buf(), Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use velyr_runtime_permissions::{Decision, Request};

    /// A canonical temp dir used as cwd, so expectations do not depend on the process cwd.
    fn cwd() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn flags(f: impl FnOnce(&mut Permissions)) -> Permissions {
        let mut p = Permissions::default();
        f(&mut p);
        p
    }

    fn list(items: &[&str]) -> Option<Vec<String>> {
        Some(items.iter().map(|s| (*s).to_owned()).collect())
    }

    fn built(p: Permissions, config: Option<&Loaded>, cwd: &Path) -> Built {
        build(p, config, cwd, Path::new("main.js")).unwrap()
    }

    fn allowed(b: &Built, request: Request<'_>) -> bool {
        b.capabilities.check(&request) == Decision::Allow
    }

    fn loaded(dir: &Path, json: &str) -> Loaded {
        Loaded {
            dir: dir.to_path_buf(),
            config: serde_json::from_str::<Config>(json).unwrap(),
        }
    }

    fn path(dir: &Path, rel: &str) -> String {
        dir.join(rel).to_string_lossy().into_owned()
    }

    #[test]
    fn allow_read_resolves_relative_to_cwd_even_if_missing() {
        let dir = cwd();
        let b = built(
            flags(|p| p.allow_read = list(&["./data"])),
            None,
            dir.path(),
        );
        assert!(allowed(
            &b,
            Request::FsRead(&path(dir.path(), "data/x.txt"))
        ));
        assert!(allowed(&b, Request::FsRead(&path(dir.path(), "data"))));
        assert!(!allowed(&b, Request::FsRead(&path(dir.path(), "data2/x"))));
        assert!(!allowed(
            &b,
            Request::FsRead(&path(dir.path(), "data/../x"))
        ));
        assert!(!allowed(&b, Request::FsWrite(&path(dir.path(), "data/x"))));
    }

    #[test]
    fn cli_flags_replace_config_permissions() {
        let dir = cwd();
        let config = loaded(dir.path(), r#"{"permissions": {"allow-env": true}}"#);
        let only_config = built(Permissions::default(), Some(&config), dir.path());
        assert!(allowed(&only_config, Request::EnvRead("HOME")));

        let b = built(
            flags(|p| p.allow_read = list(&["."])),
            Some(&config),
            dir.path(),
        );
        assert!(!allowed(&b, Request::EnvRead("HOME")));
        assert!(allowed(&b, Request::FsRead(&path(dir.path(), "a"))));
    }

    #[test]
    fn deny_flag_alone_also_replaces_config() {
        let dir = cwd();
        let config = loaded(dir.path(), r#"{"permissions": {"allow-env": true}}"#);
        let b = built(
            flags(|p| p.deny_net = list(&["x.com"])),
            Some(&config),
            dir.path(),
        );
        assert!(!allowed(&b, Request::EnvRead("HOME")));
    }

    #[test]
    fn allow_all_skips_native_and_inspector_and_warns() {
        let dir = cwd();
        let b = built(flags(|p| p.allow_all = true), None, dir.path());
        assert_eq!(b.warnings.len(), 1);
        assert!(b.warnings[0].contains("--allow-all"));
        assert!(allowed(&b, Request::FsRead(&path(dir.path(), "any"))));
        assert!(allowed(&b, Request::FsWrite(&path(dir.path(), "any"))));
        assert!(allowed(&b, Request::NetConnect("example.com", 443)));
        assert!(allowed(&b, Request::NetListen("127.0.0.1", 8080)));
        assert!(allowed(&b, Request::NetListen("::1", 8080)));
        assert!(allowed(&b, Request::EnvRead("HOME")));
        assert!(allowed(&b, Request::EnvWrite("HOME")));
        assert!(allowed(&b, Request::ProcessSpawn("git")));
        assert!(allowed(&b, Request::ImportRemote("esm.sh")));
        assert!(!allowed(&b, Request::NativeLoad(&path(dir.path(), "x.so"))));
        assert!(!allowed(&b, Request::InspectorBind("127.0.0.1")));
    }

    #[test]
    fn deny_wins_over_allow_all() {
        let dir = cwd();
        let b = built(
            flags(|p| {
                p.allow_all = true;
                p.deny_env = list(&["SECRET_*"]);
                p.deny_net = list(&["evil.com"]);
            }),
            None,
            dir.path(),
        );
        assert!(allowed(&b, Request::EnvRead("HOME")));
        assert!(!allowed(&b, Request::EnvRead("SECRET_KEY")));
        assert!(!allowed(&b, Request::EnvWrite("SECRET_KEY")));
        assert!(!allowed(&b, Request::NetConnect("evil.com", 80)));
    }

    #[test]
    fn bare_deny_blocks_the_whole_kind() {
        let dir = cwd();
        let b = built(
            flags(|p| {
                p.allow_all = true;
                p.deny_listen = Some(vec![]);
                p.deny_read = Some(vec![]);
            }),
            None,
            dir.path(),
        );
        assert!(!allowed(&b, Request::NetListen("0.0.0.0", 80)));
        assert!(!allowed(&b, Request::FsRead(&path(dir.path(), "a"))));
        assert!(allowed(&b, Request::FsWrite(&path(dir.path(), "a"))));
    }

    #[test]
    fn net_and_listen_accept_host_port_forms() {
        let dir = cwd();
        let b = built(
            flags(|p| {
                p.allow_net = list(&["api.example.com:443", "*.cdn.com", "[::1]:80"]);
                p.allow_listen = list(&["127.0.0.1:8080", "::1"]);
            }),
            None,
            dir.path(),
        );
        assert!(allowed(&b, Request::NetConnect("api.example.com", 443)));
        assert!(!allowed(&b, Request::NetConnect("api.example.com", 80)));
        assert!(allowed(&b, Request::NetConnect("a.cdn.com", 1234)));
        assert!(allowed(&b, Request::NetConnect("::1", 80)));
        assert!(allowed(&b, Request::NetListen("127.0.0.1", 8080)));
        assert!(!allowed(&b, Request::NetListen("127.0.0.1", 8081)));
        assert!(allowed(&b, Request::NetListen("::1", 9)));
    }

    #[test]
    fn env_run_import_and_native_are_scoped() {
        let dir = cwd();
        let b = built(
            flags(|p| {
                p.allow_env = list(&["DATABASE_URL", "AWS_*"]);
                p.allow_run = list(&["git"]);
                p.allow_import = list(&["esm.sh"]);
                p.allow_native = list(&["./native"]);
            }),
            None,
            dir.path(),
        );
        assert!(allowed(&b, Request::EnvRead("AWS_KEY")));
        assert!(!allowed(&b, Request::EnvRead("HOME")));
        assert!(allowed(&b, Request::ProcessSpawn("git")));
        assert!(!allowed(&b, Request::ProcessSpawn("rm")));
        assert!(allowed(&b, Request::ImportRemote("esm.sh")));
        assert!(!allowed(&b, Request::ImportRemote("evil.com")));
        assert!(allowed(
            &b,
            Request::NativeLoad(&path(dir.path(), "native/a.so"))
        ));
        assert!(!allowed(
            &b,
            Request::NativeLoad(&path(dir.path(), "other/a.so"))
        ));
    }

    #[test]
    fn inspect_defaults_to_loopback_and_drops_the_port() {
        let dir = cwd();
        let bare = built(flags(|p| p.inspect = Some(vec![])), None, dir.path());
        assert!(allowed(&bare, Request::InspectorBind("127.0.0.1")));
        assert!(!allowed(&bare, Request::InspectorBind("0.0.0.0")));
        let b = built(
            flags(|p| p.inspect = list(&["0.0.0.0:9230"])),
            None,
            dir.path(),
        );
        assert!(allowed(&b, Request::InspectorBind("0.0.0.0")));
    }

    #[test]
    fn import_local_covers_project_root_but_never_reads() {
        let dir = cwd();
        let b = built(Permissions::default(), None, dir.path());
        assert!(allowed(
            &b,
            Request::ImportLocal(&path(dir.path(), "lib/a.js"))
        ));
        assert!(!allowed(
            &b,
            Request::ImportLocal(&path(dir.path(), "../outside.js"))
        ));
        assert!(!allowed(&b, Request::FsRead(&path(dir.path(), "lib/a.js"))));
    }

    #[test]
    fn project_root_is_the_config_dir_else_the_entry_dir() {
        let dir = cwd();
        let sub = dir.path().join("sub");
        let config = loaded(dir.path(), "{}");
        let with_config = build(
            Permissions::default(),
            Some(&config),
            &sub,
            Path::new("main.js"),
        )
        .unwrap();
        assert!(allowed(
            &with_config,
            Request::ImportLocal(&path(dir.path(), "x.js"))
        ));

        let without = build(
            Permissions::default(),
            None,
            dir.path(),
            Path::new("sub/main.js"),
        )
        .unwrap();
        assert!(allowed(&without, Request::ImportLocal(&path(&sub, "x.js"))));
        assert!(!allowed(
            &without,
            Request::ImportLocal(&path(dir.path(), "x.js"))
        ));
    }

    #[test]
    fn no_flags_and_no_config_grants_nothing_else() {
        let dir = cwd();
        let b = built(Permissions::default(), None, dir.path());
        assert!(b.warnings.is_empty());
        assert!(!allowed(&b, Request::EnvRead("HOME")));
        assert!(!allowed(&b, Request::NetConnect("example.com", 443)));
    }

    #[test]
    fn config_false_and_empty_list_grant_nothing() {
        let dir = cwd();
        let config = loaded(
            dir.path(),
            r#"{"permissions": {"allow-env": false, "allow-read": [], "allow-run": ["git"]}}"#,
        );
        let b = built(Permissions::default(), Some(&config), dir.path());
        assert!(!allowed(&b, Request::EnvRead("HOME")));
        assert!(!allowed(&b, Request::FsRead(&path(dir.path(), "a"))));
        assert!(allowed(&b, Request::ProcessSpawn("git")));
    }

    #[test]
    fn quotas_come_from_the_config() {
        let dir = cwd();
        let config = loaded(dir.path(), r#"{"quotas": {"timers": 7}}"#);
        assert_eq!(
            built(Permissions::default(), Some(&config), dir.path())
                .quotas
                .timers,
            Some(7)
        );
    }

    #[test]
    fn invalid_values_name_the_flag() {
        let dir = cwd();
        for (p, flag) in [
            (flags(|p| p.allow_net = list(&["h:99999"])), "--allow-net"),
            (flags(|p| p.deny_listen = list(&["[::1"])), "--deny-listen"),
            (flags(|p| p.allow_read = list(&[""])), "--allow-read"),
            (flags(|p| p.allow_net = list(&["a:b:c"])), "--allow-net"),
        ] {
            let err = build(p, None, dir.path(), Path::new("main.js")).unwrap_err();
            assert!(err.to_string().contains(flag), "{err}");
        }
    }

    #[test]
    fn clap_maps_bare_and_listed_flags() {
        #[derive(clap::Parser)]
        struct Cli {
            #[command(flatten)]
            permissions: Permissions,
        }
        use clap::Parser;
        let p = Cli::parse_from(["x", "--allow-net", "--allow-read=a,b", "-A"]).permissions;
        assert_eq!(p.allow_net, Some(vec![]));
        assert_eq!(p.allow_read, list(&["a", "b"]));
        assert_eq!(p.allow_env, None);
        assert!(p.allow_all);
    }
}

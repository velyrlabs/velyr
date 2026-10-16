use std::collections::HashMap;
use std::fmt;
use std::io::ErrorKind;
use std::path::Path;

use reqwest::Url;
use velyr_runtime_errors::VelyrError;

const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org/";
const AUTH_TOKEN_SUFFIX: &str = ":_authToken";

/// Auth token whose value never appears in `Debug` output.
#[derive(Clone)]
struct AuthToken(String);

impl fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Parsed `.npmrc` settings relevant to the installer.
#[derive(Clone)]
pub struct Npmrc {
    registry: String,
    scope_registries: HashMap<String, String>,
    /// Keyed by nerf-dart prefix, e.g. `//host:port/path/`.
    tokens: HashMap<String, AuthToken>,
}

impl Default for Npmrc {
    fn default() -> Self {
        Self {
            registry: DEFAULT_REGISTRY.to_owned(),
            scope_registries: HashMap::new(),
            tokens: HashMap::new(),
        }
    }
}

impl fmt::Debug for Npmrc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Npmrc")
            .field("registry", &self.registry)
            .field("scope_registries", &self.scope_registries)
            .field("token_prefixes", &self.tokens.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Npmrc {
    /// Parses `.npmrc` text; `${VAR}` is expanded through `env`.
    pub fn parse(text: &str, env: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut npmrc = Self::default();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            let Some((key, raw)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let Some(value) = expand_env(raw.trim().trim_matches(['"', '\'']), env) else {
                tracing::warn!(target: "velyr.compat-npm.npmrc", key, "skipping entry with unset environment variable");
                continue;
            };
            npmrc.apply(key, value);
        }
        npmrc
    }

    /// Loads user then project `.npmrc` (project wins). Missing files are ignored.
    pub fn load(
        user: Option<&Path>,
        project: Option<&Path>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, VelyrError> {
        let mut merged = Self::default();
        for path in [user, project].into_iter().flatten() {
            match std::fs::read_to_string(path) {
                Ok(text) => merged.merge(Self::parse(&text, env)),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(VelyrError::internal("cannot read .npmrc").with_cause(e)),
            }
        }
        Ok(merged)
    }

    /// `$HOME/.npmrc`, if `HOME` is set.
    pub fn user_path() -> Option<std::path::PathBuf> {
        std::env::var_os("HOME").map(|home| Path::new(&home).join(".npmrc"))
    }

    /// Registry base URL (always ending in `/`) for a package name.
    pub fn registry_for(&self, package: &str) -> &str {
        package
            .strip_prefix('@')
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(scope, _)| self.scope_registries.get(scope))
            .unwrap_or(&self.registry)
    }

    /// Token for the longest configured prefix matching `url`'s host, port and path.
    pub fn auth_token_for(&self, url: &str) -> Option<&str> {
        let nerf = nerf_dart(&Url::parse(url).ok()?);
        self.tokens
            .iter()
            .filter(|(prefix, _)| nerf.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, token)| token.0.as_str())
    }

    fn apply(&mut self, key: &str, value: String) {
        if key == "registry" {
            self.registry = with_trailing_slash(value);
        } else if let Some(scope) = key
            .strip_suffix(":registry")
            .and_then(|k| k.strip_prefix('@'))
        {
            self.scope_registries
                .insert(scope.to_owned(), with_trailing_slash(value));
        } else if let Some(prefix) = key
            .strip_suffix(AUTH_TOKEN_SUFFIX)
            .filter(|p| p.starts_with("//"))
        {
            self.tokens
                .insert(with_trailing_slash(prefix.to_owned()), AuthToken(value));
        }
    }

    fn merge(&mut self, other: Self) {
        let default = Self::default();
        if other.registry != default.registry {
            self.registry = other.registry;
        }
        self.scope_registries.extend(other.scope_registries);
        self.tokens.extend(other.tokens);
    }
}

fn with_trailing_slash(mut value: String) -> String {
    if !value.ends_with('/') {
        value.push('/');
    }
    value
}

/// `//host[:port]/path/` form used by npm to key per-registry settings.
fn nerf_dart(url: &Url) -> String {
    let mut out = format!("//{}", url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        out.push_str(&format!(":{port}"));
    }
    out.push_str(url.path());
    with_trailing_slash(out)
}

/// Expands `${VAR}`; `None` when a referenced variable is unset.
fn expand_env(value: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let (head, tail) = rest.split_at(start);
        out.push_str(head);
        let Some(end) = tail.find('}') else {
            out.push_str(tail);
            return Some(out);
        };
        out.push_str(&env(&tail[2..end])?);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

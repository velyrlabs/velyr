//! ESM module resolution, canonical IDs, permission-gated loading and a source cache.
//!
//! Bare specifiers resolve only through a registered `BareSpecifierResolver` (npm lives in
//! `velyr-compat-npm`). Known limits: `https:` is not supported yet; the source cache
//! never invalidates (a changed file is served stale until the host is rebuilt); resolution
//! checks that the target exists before permissions are consulted, so existence of files
//! outside the grant is observable through `NotFound` vs `PermissionDenied`.

mod embedded;
mod host;
mod realpath_cache;
mod resolve;
mod shared_cache;
mod source_maps;
mod strategy;
mod transform;

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use url::Url;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_permissions::CapabilitySet;

pub use embedded::{HELPER_MODULE_NAME, OXC_HELPERS_VERSION, is_embedded};
pub use host::{FsLoader, ModuleHost, VirtualLoader, authorize_path};
pub use realpath_cache::RealpathCache;
pub use resolve::FsResolver;
pub use shared_cache::SharedCache;
pub use source_maps::{PositionMapper, SourceMaps};
pub use strategy::{BareSpecifierResolver, StrategyResolver};
pub use transform::{MediaType, SourceTransform, Transformed};

pub type Result<T> = std::result::Result<T, VelyrError>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Canonical module identity: the `file://` URL of the realpath.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModuleId(String);

impl ModuleId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `path` must be absolute; it is not canonicalized here.
    pub fn from_path(path: &Path) -> Option<Self> {
        if let Some(text) = path.to_str().filter(|text| is_plain_virtual(text)) {
            return Some(Self(format!("file://{text}")));
        }
        Url::from_file_path(path).ok().map(|url| Self(url.into()))
    }

    /// Parses a `file://` URL as produced by [`ModuleId::as_str`].
    pub fn parse(text: &str) -> Option<Self> {
        let url = Url::parse(text).ok()?;
        if virtual_path(&url).is_none() {
            url.to_file_path().ok()?;
        }
        Some(Self(url.into()))
    }

    pub fn to_path(&self) -> Option<PathBuf> {
        let url = Url::parse(&self.0).ok()?;
        match virtual_path(&url) {
            Some(path) => Some(PathBuf::from(path)),
            None => url.to_file_path().ok(),
        }
    }
}

/// Root of the embedded modules (`/$velyr/node/...`, `/$velyr/oxc-runtime/...`). It is not a
/// drive path, so Windows rejects it as a file URL and the id is built by hand.
const VIRTUAL_ROOT: &str = "/$velyr/";

/// shortcut: only unreserved characters, so the URL needs no percent coding; any other
/// embedded path falls back to the OS rules (unix only), upgrade when Windows needs them.
fn is_plain_virtual(path: &str) -> bool {
    path.strip_prefix(VIRTUAL_ROOT).is_some_and(|rest| {
        rest.split('/').all(|segment| {
            !matches!(segment, "" | "." | "..")
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._$~-".contains(c))
        })
    })
}

fn virtual_path(url: &Url) -> Option<&str> {
    (url.scheme() == "file" && url.host().is_none())
        .then(|| url.path())
        .filter(|path| is_plain_virtual(path))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModule {
    pub id: ModuleId,
    /// Canonical filesystem path; the only path the loader may open.
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSource {
    /// Shared with the engine, which may read it in place (`Isolate::compile_module_shared`).
    pub text: Arc<str>,
    /// Lowercase hex sha256 of `text`; input to future transform cache keys.
    pub hash: String,
}

impl ModuleSource {
    pub fn new(text: String) -> Self {
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        Self {
            text: Arc::from(text),
            hash,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResolveContext {
    /// `imports` of `velyr.json`, already parsed.
    pub import_map: HashMap<String, String>,
    /// Directory of the import map; relative values resolve against it.
    pub import_map_dir: PathBuf,
    /// Per-epoch realpath memo; `None` canonicalizes straight from the filesystem.
    pub realpaths: Option<Arc<RealpathCache>>,
}

#[derive(Debug, Clone)]
pub struct LoadContext {
    pub permissions: Arc<CapabilitySet>,
}

pub trait ModuleResolver {
    /// No network I/O; touches the filesystem only to canonicalize.
    fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleId,
        ctx: &ResolveContext,
    ) -> Result<ResolvedModule>;
}

pub trait ModuleLoader: Send + Sync {
    fn load<'a>(
        &'a self,
        module: &'a ResolvedModule,
        ctx: &'a LoadContext,
    ) -> BoxFuture<'a, Result<ModuleSource>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_ids_round_trip_on_every_os() {
        for text in [
            "/$velyr/oxc-runtime/helpers/decorate.js",
            "/$velyr/node/fs/promises.js",
        ] {
            let id = ModuleId::from_path(Path::new(text)).unwrap();
            assert_eq!(id.as_str(), format!("file://{text}"));
            assert_eq!(id.to_path().unwrap(), PathBuf::from(text));
            assert_eq!(ModuleId::parse(id.as_str()).unwrap(), id);
            #[cfg(unix)]
            assert_eq!(id.as_str(), Url::from_file_path(text).unwrap().as_str());
        }
    }

    #[test]
    fn virtual_paths_with_reserved_characters_use_the_os_rules() {
        for text in ["/$velyr/a b/c#d%e.js", "/$velyr/caf\u{e9}.js"] {
            let id = ModuleId::from_path(Path::new(text));
            #[cfg(unix)]
            {
                let id = id.unwrap();
                assert_eq!(id.as_str(), Url::from_file_path(text).unwrap().as_str());
                assert_eq!(id.to_path().unwrap(), PathBuf::from(text));
            }
            #[cfg(windows)]
            assert!(id.is_none(), "{text}");
        }
    }
}

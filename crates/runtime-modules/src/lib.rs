//! ESM module resolution, canonical IDs, permission-gated loading and a source cache.
//!
//! Bare specifiers resolve only through a registered `BareSpecifierResolver` (npm lives in
//! `velyr-compat-npm`). Known limits: `https:` is not supported yet; the source cache
//! never invalidates (a changed file is served stale until the host is rebuilt); resolution
//! checks that the target exists before permissions are consulted, so existence of files
//! outside the grant is observable through `NotFound` vs `PermissionDenied`.

mod embedded;
mod host;
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
        Url::from_file_path(path).ok().map(|url| Self(url.into()))
    }

    /// Parses a `file://` URL as produced by [`ModuleId::as_str`].
    pub fn parse(text: &str) -> Option<Self> {
        let url = Url::parse(text).ok()?;
        url.to_file_path().ok()?;
        Some(Self(url.into()))
    }

    pub fn to_path(&self) -> Option<PathBuf> {
        Url::parse(&self.0).ok()?.to_file_path().ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModule {
    pub id: ModuleId,
    /// Canonical filesystem path; the only path the loader may open.
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSource {
    pub text: String,
    /// Lowercase hex sha256 of `text`; input to future transform cache keys.
    pub hash: String,
}

impl ModuleSource {
    pub fn new(text: String) -> Self {
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        Self { text, hash }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResolveContext {
    /// `imports` of `velyr.json`, already parsed.
    pub import_map: HashMap<String, String>,
    /// Directory of the import map; relative values resolve against it.
    pub import_map_dir: PathBuf,
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

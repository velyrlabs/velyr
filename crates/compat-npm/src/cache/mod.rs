//! Global content-addressed npm cache: `<root>/<sha512-hex>/` holds one extracted package.
//!
//! Entries are staged in `tmp-*` next to the final directory and moved in with one `rename`,
//! so a crash or a rejected tarball never leaves a partial final directory.

mod extract;

use std::future::Future;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tracing::Instrument;
use velyr_runtime_errors::VelyrError;

pub use extract::ExtractLimits;

use crate::registry::integrity::sha512_digest;

const META_FILE: &str = ".velyr-meta.json";
const META_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Meta {
    version: u32,
    sha512: String,
    files: u64,
    bytes: u64,
}

#[derive(Debug, Clone)]
pub struct NpmCache {
    root: PathBuf,
    limits: ExtractLimits,
}

impl NpmCache {
    /// Cache rooted at `root` (the directory that contains the `<sha512-hex>` entries).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            limits: ExtractLimits::default(),
        }
    }

    /// Per-user cache at `<platform cache dir>/velyr/npm`.
    pub fn default_location() -> Result<Self, VelyrError> {
        let dirs = ProjectDirs::from("", "", "velyr")
            .ok_or_else(|| VelyrError::internal("cannot resolve the user cache directory"))?;
        Ok(Self::at(dirs.cache_dir().join("npm")))
    }

    #[must_use]
    pub fn with_limits(mut self, limits: ExtractLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Returns the extracted package directory for `integrity` (an SRI string with sha512).
    ///
    /// When a valid entry exists `fetch` is never called. Otherwise `fetch` must write the
    /// tarball to the path it receives; the bytes are verified against `integrity` and
    /// extracted safely before the entry becomes visible.
    pub async fn ensure<F, Fut>(&self, integrity: &str, fetch: F) -> Result<PathBuf, VelyrError>
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: Future<Output = Result<(), VelyrError>>,
    {
        let digest = sha512_digest(integrity)
            .ok_or_else(|| VelyrError::invalid_data("integrity has no sha512 digest"))?;
        let hex = to_hex(&digest);
        let span = tracing::info_span!("velyr.compat-npm.cache", sha512 = %hex);
        self.ensure_entry(digest, hex, fetch).instrument(span).await
    }

    async fn ensure_entry<F, Fut>(
        &self,
        digest: Vec<u8>,
        hex: String,
        fetch: F,
    ) -> Result<PathBuf, VelyrError>
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: Future<Output = Result<(), VelyrError>>,
    {
        let final_dir = self.root.join(&hex);
        if is_valid_entry(&final_dir, &hex) {
            tracing::debug!("cache hit");
            return Ok(final_dir);
        }
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| io_error("cannot create cache root", e))?;
        let staging = tempfile::Builder::new()
            .prefix("tmp-")
            .tempdir_in(&self.root)
            .map_err(|e| io_error("cannot create staging directory", e))?;
        let tarball = staging.path().join("download.tgz");
        fetch(tarball.clone()).await?;

        let extracted = staging.path().join("package");
        let limits = self.limits;
        let task_hex = hex.clone();
        let span = tracing::info_span!("velyr.compat-npm.extract");
        let stats = tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            extract::verify_and_extract(&tarball, &digest, &extracted, limits)
        })
        .await
        .map_err(|e| VelyrError::internal("extraction task failed").with_cause(e))??;

        let package_dir = staging.path().join("package");
        let meta = Meta {
            version: META_VERSION,
            sha512: task_hex,
            files: stats.files,
            bytes: stats.bytes,
        };
        let json = serde_json::to_vec(&meta)
            .map_err(|e| VelyrError::internal("cannot encode cache metadata").with_cause(e))?;
        std::fs::write(package_dir.join(META_FILE), json)
            .map_err(|e| io_error("cannot write cache metadata", e))?;
        publish(&package_dir, &final_dir, &hex)?;
        Ok(final_dir)
    }
}

/// Moves a fully built directory into place; a concurrent winner is reused.
fn publish(staged: &Path, final_dir: &Path, hex: &str) -> Result<(), VelyrError> {
    if std::fs::rename(staged, final_dir).is_ok() {
        return Ok(());
    }
    if is_valid_entry(final_dir, hex) {
        return Ok(());
    }
    // An invalid entry at the final path (corrupt metadata) is replaced.
    std::fs::remove_dir_all(final_dir).map_err(|e| io_error("cannot replace invalid entry", e))?;
    std::fs::rename(staged, final_dir).map_err(|e| io_error("cannot publish cache entry", e))
}

fn is_valid_entry(dir: &Path, hex: &str) -> bool {
    std::fs::read(dir.join(META_FILE))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Meta>(&raw).ok())
        .is_some_and(|meta| meta.version == META_VERSION && meta.sha512 == hex)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn io_error(message: &str, cause: std::io::Error) -> VelyrError {
    VelyrError::internal(message).with_cause(cause)
}

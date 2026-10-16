//! Pure dependency resolution: root `package.json` + packument lookup -> hoisted `InstallPlan`.
//!
//! No I/O happens here; see ADR-013 for the resolution rules.

mod graph;
mod hoist;
mod platform;

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;
use velyr_runtime_errors::VelyrError;

pub use platform::Platform;

use crate::PackageJson;
use crate::registry::Packument;

pub trait PackumentSource {
    /// Sync on purpose: keeps the resolver pure; an async adapter prefetches packuments first.
    fn packument(&self, name: &str) -> Result<Arc<Packument>, VelyrError>;
}

#[derive(Debug, Clone)]
pub struct ResolveOptions {
    pub platform: Platform,
    /// Turn peer dependency conflicts into errors instead of warnings.
    pub strict_peer_deps: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedPackage {
    pub name: String,
    pub version: String,
    pub tarball: String,
    pub integrity: Option<String>,
    pub has_install_script: bool,
    /// The package's own edges and platform filter, as the lockfile records them.
    pub dependencies: BTreeMap<String, String>,
    pub optional_dependencies: BTreeMap<String, String>,
    /// Required peers only; optional ones never become edges.
    pub peer_dependencies: BTreeMap<String, String>,
    pub os: Vec<String>,
    pub cpu: Vec<String>,
    /// `bin name -> path in the package`, sanitized.
    pub bin: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanWarning {
    OptionalSkipped {
        package: String,
        spec: String,
        required_by: String,
        reason: String,
    },
    PeerConflict {
        /// `name@version` that declares the peer range.
        package: String,
        peer: String,
        range: String,
        found: String,
        required_by: String,
    },
}

/// Keys of `packages` are install paths such as `node_modules/a/node_modules/b`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallPlan {
    pub packages: BTreeMap<String, PlannedPackage>,
    pub warnings: Vec<PlanWarning>,
}

pub fn resolve(
    root: &PackageJson,
    source: &dyn PackumentSource,
    options: &ResolveOptions,
) -> Result<InstallPlan, VelyrError> {
    let _span = tracing::info_span!("velyr.compat_npm.resolve").entered();
    let (graph, warnings) = graph::build(root, source, options)?;
    Ok(InstallPlan {
        packages: hoist::place(&graph),
        warnings,
    })
}

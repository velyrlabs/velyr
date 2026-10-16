use std::collections::BTreeMap;

use serde::Deserialize;

use crate::PeerDependencyMeta;

/// Abbreviated packument (`application/vnd.npm.install-v1+json`).
#[derive(Debug, Clone, Deserialize)]
pub struct Packument {
    pub name: String,
    #[serde(rename = "dist-tags", default)]
    pub dist_tags: BTreeMap<String, String>,
    #[serde(default)]
    pub versions: BTreeMap<String, VersionManifest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionManifest {
    pub version: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(rename = "peerDependencies", default)]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(rename = "peerDependenciesMeta", default)]
    pub peer_dependencies_meta: BTreeMap<String, PeerDependencyMeta>,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default)]
    pub cpu: Vec<String>,
    #[serde(default)]
    pub deprecated: Option<String>,
    #[serde(rename = "hasInstallScript", default)]
    pub has_install_script: bool,
    /// Raw `bin`: a path or a name -> path map.
    #[serde(default)]
    pub bin: Option<serde_json::Value>,
    pub dist: Dist,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dist {
    pub tarball: String,
    /// SRI string, possibly with several space-separated entries.
    #[serde(default)]
    pub integrity: Option<String>,
    /// Legacy hex sha1.
    #[serde(default)]
    pub shasum: Option<String>,
}

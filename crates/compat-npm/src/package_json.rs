//! Tolerant typed model of an npm `package.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

type StringMap = BTreeMap<String, String>;

/// Entry of `peerDependenciesMeta`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PeerDependencyMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Fields with polymorphic shapes (`exports`, `imports`, `bin`, `workspaces`) stay as raw JSON;
/// resolving them is the job of later layers. Unknown fields are kept in `extra`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PackageJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exports: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imports: Option<Value>,
    #[serde(default, skip_serializing_if = "StringMap::is_empty")]
    pub dependencies: StringMap,
    #[serde(
        default,
        rename = "devDependencies",
        skip_serializing_if = "StringMap::is_empty"
    )]
    pub dev_dependencies: StringMap,
    #[serde(
        default,
        rename = "peerDependencies",
        skip_serializing_if = "StringMap::is_empty"
    )]
    pub peer_dependencies: StringMap,
    #[serde(
        default,
        rename = "peerDependenciesMeta",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub peer_dependencies_meta: BTreeMap<String, PeerDependencyMeta>,
    #[serde(
        default,
        rename = "optionalDependencies",
        skip_serializing_if = "StringMap::is_empty"
    )]
    pub optional_dependencies: StringMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<Value>,
    #[serde(default, skip_serializing_if = "StringMap::is_empty")]
    pub scripts: StringMap,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub os: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu: Vec<String>,
    #[serde(default, skip_serializing_if = "StringMap::is_empty")]
    pub engines: StringMap,
    /// Parsed only; workspace support is out of scope for v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspaces: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PackageJson {
    pub fn parse(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw)
    }
}

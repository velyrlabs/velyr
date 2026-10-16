//! `velyr.lock`: deterministic JSON lockfile (ADR-013).
//!
//! `serde_json` runs with `preserve_order` workspace-wide, so determinism comes from typed
//! structs (fields declared in alphabetical order) and `BTreeMap` for every map, never from `Value`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use velyr_runtime_errors::VelyrError;

use crate::PackageJson;
use crate::registry::{Dist, Packument, VersionManifest};

pub const LOCKFILE_VERSION: u32 = 1;
const ROOT_IMPORTER: &str = ".";

type StringMap = BTreeMap<String, String>;

/// Direct dependency specs of one importer, exactly as written in its `package.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Importer {
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
        rename = "optionalDependencies",
        skip_serializing_if = "StringMap::is_empty"
    )]
    pub optional_dependencies: StringMap,
}

/// One resolved `name@version` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    /// `bin name -> path in the package`.
    #[serde(default, skip_serializing_if = "StringMap::is_empty")]
    pub bin: StringMap,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu: Vec<String>,
    #[serde(default, skip_serializing_if = "StringMap::is_empty")]
    pub dependencies: StringMap,
    #[serde(
        default,
        rename = "hasInstallScript",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub has_install_script: bool,
    /// SRI `sha512` hash.
    pub integrity: String,
    #[serde(
        default,
        rename = "optionalDependencies",
        skip_serializing_if = "StringMap::is_empty"
    )]
    pub optional_dependencies: StringMap,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub os: Vec<String>,
    /// Required peers only (the ones resolved as edges).
    #[serde(
        default,
        rename = "peerDependencies",
        skip_serializing_if = "StringMap::is_empty"
    )]
    pub peer_dependencies: StringMap,
    pub resolved: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lockfile {
    /// Every package carries its graph (edges, platform, bin), so the tree can be rebuilt
    /// without a registry. Older locks lack it and are resolved again, pinned.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub complete: bool,
    pub importers: BTreeMap<String, Importer>,
    #[serde(rename = "lockfileVersion")]
    pub lockfile_version: u32,
    pub packages: BTreeMap<String, LockedPackage>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            complete: false,
            importers: BTreeMap::new(),
            lockfile_version: LOCKFILE_VERSION,
            packages: BTreeMap::new(),
        }
    }
}

impl Lockfile {
    pub fn parse(raw: &str) -> Result<Self, VelyrError> {
        let value: serde_json::Value = serde_json::from_str(raw).map_err(invalid)?;
        match value.get("lockfileVersion").and_then(|v| v.as_u64()) {
            Some(v) if v == u64::from(LOCKFILE_VERSION) => {}
            other => {
                return Err(VelyrError::invalid_data(format!(
                    "unsupported lockfileVersion {}, expected {LOCKFILE_VERSION}",
                    other.map_or_else(|| "(missing)".to_owned(), |v| v.to_string())
                )));
            }
        }
        serde_json::from_value(value).map_err(invalid)
    }

    /// The lock as a pre-graph version of velyr would have written it.
    pub fn without_graph(&self) -> Self {
        let mut lock = self.clone();
        lock.complete = false;
        for package in lock.packages.values_mut() {
            *package = LockedPackage {
                has_install_script: package.has_install_script,
                integrity: std::mem::take(&mut package.integrity),
                resolved: std::mem::take(&mut package.resolved),
                ..LockedPackage::default()
            };
        }
        lock
    }

    /// Packuments rebuilt from the locked graph, holding only the locked versions: a resolver
    /// run over them needs no registry. `None` unless the lock is [`complete`](Self::complete).
    pub fn packuments(&self) -> Option<BTreeMap<String, Arc<Packument>>> {
        if !self.complete {
            return None;
        }
        let mut packuments: BTreeMap<String, Packument> = BTreeMap::new();
        for (key, locked) in &self.packages {
            let (name, version) = key.rsplit_once('@')?;
            packuments
                .entry(name.to_owned())
                .or_insert_with(|| Packument {
                    name: name.to_owned(),
                    dist_tags: BTreeMap::new(),
                    versions: BTreeMap::new(),
                })
                .versions
                .insert(version.to_owned(), locked.manifest(version));
        }
        Some(
            packuments
                .into_iter()
                .map(|(k, v)| (k, Arc::new(v)))
                .collect(),
        )
    }

    /// Two-space JSON with sorted keys and a trailing newline.
    pub fn render(&self) -> String {
        let mut out = serde_json::to_string_pretty(self).expect("lockfile always serializes");
        out.push('\n');
        out
    }

    pub fn read(path: &Path) -> Result<Self, VelyrError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| VelyrError::not_found(format!("cannot read {}: {e}", path.display())))?;
        Self::parse(&raw)
    }

    pub fn write(&self, path: &Path) -> Result<(), VelyrError> {
        std::fs::write(path, self.render())
            .map_err(|e| VelyrError::internal(format!("cannot write {}: {e}", path.display())))
    }

    /// Whether the root importer's direct specs match `package_json` exactly.
    pub fn is_satisfied_by(&self, package_json: &PackageJson) -> bool {
        self.drift(package_json).is_empty()
    }

    /// `--frozen`: fails on any divergence, listing each affected package.
    pub fn check_frozen(&self, package_json: &PackageJson) -> Result<(), VelyrError> {
        let drift = self.drift(package_json);
        if drift.is_empty() {
            return Ok(());
        }
        Err(VelyrError::invalid_data(format!(
            "velyr.lock is out of date with package.json:\n{}",
            drift.join("\n")
        )))
    }

    fn drift(&self, package_json: &PackageJson) -> Vec<String> {
        let empty = Importer::default();
        let locked = self.importers.get(ROOT_IMPORTER).unwrap_or(&empty);
        let sections = [
            (
                "dependencies",
                &locked.dependencies,
                &package_json.dependencies,
            ),
            (
                "devDependencies",
                &locked.dev_dependencies,
                &package_json.dev_dependencies,
            ),
            (
                "optionalDependencies",
                &locked.optional_dependencies,
                &package_json.optional_dependencies,
            ),
        ];
        sections
            .into_iter()
            .flat_map(|(section, lock, pkg)| diff_section(section, lock, pkg))
            .collect()
    }
}

impl LockedPackage {
    fn manifest(&self, version: &str) -> VersionManifest {
        VersionManifest {
            version: version.to_owned(),
            dependencies: self.dependencies.clone(),
            optional_dependencies: self.optional_dependencies.clone(),
            peer_dependencies: self.peer_dependencies.clone(),
            peer_dependencies_meta: BTreeMap::new(),
            os: self.os.clone(),
            cpu: self.cpu.clone(),
            deprecated: None,
            has_install_script: self.has_install_script,
            bin: (!self.bin.is_empty()).then(|| serde_json::json!(self.bin)),
            dist: Dist {
                tarball: self.resolved.clone(),
                integrity: Some(self.integrity.clone()),
                shasum: None,
            },
        }
    }
}

fn diff_section(section: &str, lock: &StringMap, pkg: &StringMap) -> Vec<String> {
    let mut lines = Vec::new();
    for (name, spec) in pkg {
        match lock.get(name) {
            None => lines.push(format!("  + {name}@{spec} added ({section})")),
            Some(locked) if locked != spec => {
                lines.push(format!(
                    "  ~ {name}: lock has {locked}, package.json has {spec} ({section})"
                ));
            }
            Some(_) => {}
        }
    }
    for (name, spec) in lock {
        if !pkg.contains_key(name) {
            lines.push(format!("  - {name}@{spec} removed ({section})"));
        }
    }
    lines
}

fn invalid(e: serde_json::Error) -> VelyrError {
    VelyrError::invalid_data(format!("malformed velyr.lock: {e}"))
}

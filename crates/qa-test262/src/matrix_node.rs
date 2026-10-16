//! Node behavior and npm package half of `gen-compat-matrix`: loads `node-behavior-*.json` and
//! `node-packages-*.json` from `compat/results/` and renders their tables (included by the bin).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use super::{Counts, cell};

/// Newest `node-behavior-<module>-*.json` of one Node module.
pub struct Behavior {
    pub total: usize,
    pub pass: usize,
    pub evidence: String,
}

/// Newest install + smoke result of one npm package.
pub struct Package {
    pub version: String,
    pub installed: bool,
    /// `pass`, `fail` or `skip`.
    pub smoke: String,
    /// The smoke could only cover import and construction (needs a server).
    pub partial: bool,
    pub evidence: String,
}

#[derive(Default)]
pub struct NodeResults {
    pub behavior: BTreeMap<String, Behavior>,
    pub packages: BTreeMap<String, Package>,
}

/// A small sample cannot prove `supported`: that status belongs to suite evidence from CI.
pub fn behavior_status(pass: usize, total: usize) -> &'static str {
    match (pass, total) {
        (0, _) => "unsupported",
        (p, t) if p < t => "partial",
        _ => "experimental",
    }
}

pub fn package_status(package: &Package) -> &'static str {
    match (
        package.installed && package.smoke == "pass",
        package.partial,
    ) {
        (false, _) => "unsupported",
        (true, true) => "partial (no server)",
        (true, false) => "experimental",
    }
}

/// Result files `<prefix>*.json` of `dir`, oldest first by `timestamp`, then file name.
pub fn runs(dir: &Path, prefix: &str) -> Result<Vec<(String, Value)>, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut found: Vec<(u64, String, Value)> = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if !name.starts_with(prefix) || !name.ends_with(".json") {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        found.push((doc["timestamp"].as_u64().unwrap_or(0), name, doc));
    }
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    Ok(found.into_iter().map(|(_, n, d)| (n, d)).collect())
}

pub fn load_node(dir: &Path) -> Result<NodeResults, String> {
    let mut node = NodeResults::default();
    for (name, doc) in runs(dir, "node-behavior-")? {
        if let Some(module) = doc["module"].as_str() {
            node.behavior.insert(
                module.to_owned(),
                Behavior {
                    total: doc["total"].as_u64().unwrap_or(0) as usize,
                    pass: doc["passed"].as_u64().unwrap_or(0) as usize,
                    evidence: name,
                },
            );
        }
    }
    for (name, doc) in runs(dir, "node-packages-")? {
        for (pkg, entry) in doc["packages"].as_object().into_iter().flatten() {
            node.packages.insert(
                pkg.clone(),
                Package {
                    version: entry["version"].as_str().unwrap_or("-").to_owned(),
                    installed: entry["installed"].as_bool().unwrap_or(false),
                    smoke: entry["smoke"].as_str().unwrap_or("skip").to_owned(),
                    partial: entry["partial"].as_bool().unwrap_or(false),
                    evidence: name.clone(),
                },
            );
        }
    }
    Ok(node)
}

/// Smoke script of a package under `compat/node/smoke/`: `@nestjs/core` -> `nestjs-core.mjs`.
fn smoke_file(package: &str) -> String {
    format!("{}.mjs", package.trim_start_matches('@').replace('/', "-"))
}

/// One summary status for a set of per-item statuses.
pub fn aggregate<'a>(statuses: impl Iterator<Item = &'a str>) -> &'static str {
    let all: Vec<&str> = statuses.collect();
    if all.is_empty() {
        "planned"
    } else if all.iter().all(|s| *s == "unsupported") {
        "unsupported"
    } else if all.iter().all(|s| *s == "experimental") {
        "experimental"
    } else {
        "partial"
    }
}

pub fn render_node(node: &NodeResults, out: &mut String) {
    out.push_str("## Node core modules\n\n");
    out.push_str("| Module | Behavior tests | Status | Evidence |\n| --- | --- | --- | --- |\n");
    for (module, b) in &node.behavior {
        let counts = Some(Counts {
            pass: b.pass,
            total: b.total,
        });
        out.push_str(&format!(
            "| {module} | {} | {} | [result](../compat/results/{}) |\n",
            cell(counts),
            behavior_status(b.pass, b.total),
            b.evidence
        ));
    }
    out.push_str("\n## npm packages\n\n");
    out.push_str(
        "Smoke only: install without lifecycle scripts, import, exercise the main API. \
         The package test suites are not run.\n\n",
    );
    out.push_str("| Package | Version | Status | Evidence |\n| --- | --- | --- | --- |\n");
    for (name, p) in &node.packages {
        out.push_str(&format!(
            "| {name} | {} | {} | [result](../compat/results/{}), [smoke](../compat/node/smoke/{}) |\n",
            p.version,
            package_status(p),
            p.evidence,
            smoke_file(name)
        ));
    }
    out.push('\n');
}

/// Status per Node core module and npm package, embedded by `velyr compat`.
pub fn render_json(node: &NodeResults) -> String {
    let modules: BTreeMap<&str, &str> = node
        .behavior
        .iter()
        .map(|(name, b)| (name.as_str(), behavior_status(b.pass, b.total)))
        .collect();
    let packages: BTreeMap<&str, &str> = node
        .packages
        .iter()
        .map(|(name, p)| (name.as_str(), package_status(p)))
        .collect();
    let json = serde_json::json!({ "modules": modules, "packages": packages });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&json).unwrap_or_default()
    )
}

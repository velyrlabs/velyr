//! Materializes an `InstallPlan` into `node_modules` (ADR-013): hardlinks from the global cache,
//! incremental against a state file, `.bin` links for top-level packages.

mod bin;
mod tree;

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use velyr_runtime_errors::VelyrError;

const STATE_FILE: &str = ".velyr-state.json";

/// The sanitized `bin name -> path` pairs of a package's raw `bin` field.
pub fn declared_bins(package_name: &str, bin: &Value) -> BTreeMap<String, String> {
    bin::declared(package_name, &serde_json::json!({ "bin": bin }))
}

/// One package to place: `path` is an install path such as `node_modules/a/node_modules/b`.
#[derive(Debug, Clone)]
pub struct LinkEntry {
    pub path: String,
    pub name: String,
    pub version: String,
    pub integrity: String,
    /// Extracted package directory in the global cache.
    pub source: PathBuf,
}

impl LinkEntry {
    fn fingerprint(&self) -> String {
        format!("{}@{}#{}", self.name, self.version, self.integrity)
    }
    fn label(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LinkReport {
    /// `name@version` of every package materialized by this run.
    pub added: Vec<String>,
    /// `name@version` of every package that left the plan or was replaced.
    pub removed: Vec<String>,
}

#[derive(Default, Serialize, Deserialize, PartialEq, Eq)]
struct State {
    /// install path -> fingerprint
    packages: BTreeMap<String, String>,
    /// bin name -> link target relative to `.bin`
    bins: BTreeMap<String, String>,
}

/// Brings `<project>/node_modules` in line with `entries`, touching only what changed.
pub fn link(project: &Path, entries: &[LinkEntry]) -> Result<LinkReport, VelyrError> {
    link_with(project, entries, &tree::hardlink)
}

fn link_with(
    project: &Path,
    entries: &[LinkEntry],
    linker: &dyn Fn(&Path, &Path) -> io::Result<()>,
) -> Result<LinkReport, VelyrError> {
    for entry in entries {
        validate_path(&entry.path)?;
    }
    let modules = project.join("node_modules");
    let old = read_state(&modules);
    let mut state = State {
        packages: old.packages.clone(),
        bins: old.bins.clone(),
    };
    let mut report = LinkReport::default();

    let planned: BTreeMap<&str, &LinkEntry> =
        entries.iter().map(|e| (e.path.as_str(), e)).collect();
    // Anything under a replaced package is wiped with it, so it must be relinked too.
    let mut changed: BTreeSet<&str> = BTreeSet::new();
    for (path, entry) in &planned {
        let same = old.packages.get(*path) == Some(&entry.fingerprint())
            && project.join(path).is_dir()
            && !changed.iter().any(|c| descends(path, c));
        if !same {
            changed.insert(path);
        }
    }

    for (path, fingerprint) in &old.packages {
        if !planned.contains_key(path.as_str()) || changed.contains(path.as_str()) {
            remove_package(project, path)?;
            state.packages.remove(path);
            report.removed.push(label_of(fingerprint));
        }
    }
    for path in &changed {
        let entry = planned[path];
        let dst = project.join(path);
        let _ = std::fs::remove_dir_all(&dst);
        tree::link_tree(&entry.source, &dst, linker)?;
        state
            .packages
            .insert((*path).to_owned(), entry.fingerprint());
        report.added.push(entry.label());
    }

    sync_bins(project, &planned, &mut state)?;
    if state != old || !modules.join(STATE_FILE).exists() {
        write_state(&modules, &state)?;
    }
    Ok(report)
}

fn descends(path: &str, ancestor: &str) -> bool {
    path.len() > ancestor.len()
        && path.starts_with(ancestor)
        && path.as_bytes()[ancestor.len()] == b'/'
}

fn label_of(fingerprint: &str) -> String {
    fingerprint
        .split('#')
        .next()
        .unwrap_or(fingerprint)
        .to_owned()
}

/// Install paths come from registry data: only `node_modules/<segment>...` is accepted.
fn validate_path(path: &str) -> Result<(), VelyrError> {
    let ok = Path::new(path).starts_with("node_modules")
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if ok {
        Ok(())
    } else {
        Err(VelyrError::invalid_data(format!(
            "unsafe install path `{path}`"
        )))
    }
}

fn remove_package(project: &Path, path: &str) -> Result<(), VelyrError> {
    let dir = project.join(path);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(
                VelyrError::internal(format!("cannot remove {}", dir.display())).with_cause(e),
            );
        }
    }
    // Prune scope directories and nested `node_modules` left empty (`remove_dir` fails if not).
    let mut parent = dir.parent();
    while let Some(p) = parent {
        if p == project.join("node_modules") || std::fs::remove_dir(p).is_err() {
            break;
        }
        parent = p.parent();
    }
    Ok(())
}

fn sync_bins(
    project: &Path,
    planned: &BTreeMap<&str, &LinkEntry>,
    state: &mut State,
) -> Result<(), VelyrError> {
    let bin_dir = project.join("node_modules/.bin");
    let mut desired = BTreeMap::new();
    for entry in planned.values() {
        if entry.path != format!("node_modules/{}", entry.name) {
            continue; // nested copies do not own `.bin` names
        }
        let Ok(raw) = std::fs::read_to_string(project.join(&entry.path).join("package.json"))
        else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        for (name, target) in bin::declared(&entry.name, &manifest) {
            desired.insert(name, format!("../{}/{target}", entry.name));
        }
    }
    let present = |name: &str| {
        bin_dir.join(name).symlink_metadata().is_ok()
            || bin_dir.join(format!("{name}.cmd")).exists()
    };
    for name in state.bins.keys().cloned().collect::<Vec<_>>() {
        if !desired.contains_key(&name) {
            bin::remove(&bin_dir, &name);
            state.bins.remove(&name);
        }
    }
    for (name, target) in desired {
        if state.bins.get(&name) == Some(&target) && present(&name) {
            // A relinked package got a fresh, non-executable file under the same bin.
            bin::ensure_executable(&bin_dir.join(&name));
            continue;
        }
        bin::create(&bin_dir, &name, &target).map_err(|e| {
            VelyrError::internal(format!("cannot create .bin/{name}")).with_cause(e)
        })?;
        state.bins.insert(name, target);
    }
    if state.bins.is_empty() {
        let _ = std::fs::remove_dir(&bin_dir);
    }
    Ok(())
}

fn read_state(modules: &Path) -> State {
    std::fs::read_to_string(modules.join(STATE_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_state(modules: &Path, state: &State) -> Result<(), VelyrError> {
    std::fs::create_dir_all(modules)
        .and_then(|()| std::fs::write(modules.join(STATE_FILE), serde_json::to_vec_pretty(state)?))
        .map_err(|e| VelyrError::internal("cannot write node_modules state").with_cause(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(dir: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = dir.join("cache").join(name.replace('/', "_"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".velyr-meta.json"), "{}").unwrap();
        for (file, body) in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        root
    }

    fn entry(path: &str, name: &str, source: PathBuf) -> LinkEntry {
        LinkEntry {
            path: path.to_owned(),
            name: name.to_owned(),
            version: "1.0.0".to_owned(),
            integrity: "sha512-x".to_owned(),
            source,
        }
    }

    #[test]
    fn falls_back_to_copy_when_hardlink_fails() {
        let dir = tempfile::tempdir().unwrap();
        let src = cached(dir.path(), "a", &[("index.js", "1"), ("lib/x.js", "2")]);
        let fail = |_: &Path, _: &Path| Err(io::Error::from(io::ErrorKind::CrossesDevices));
        let project = dir.path().join("p");
        link_with(&project, &[entry("node_modules/a", "a", src)], &fail).unwrap();
        let out = project.join("node_modules/a");
        assert_eq!(std::fs::read_to_string(out.join("lib/x.js")).unwrap(), "2");
        assert!(!out.join(".velyr-meta.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn hardlinks_share_the_cache_inode() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let src = cached(dir.path(), "a", &[("index.js", "1")]);
        let project = dir.path().join("p");
        link(&project, &[entry("node_modules/a", "a", src.clone())]).unwrap();
        let ino = |p: PathBuf| std::fs::metadata(p).unwrap().ino();
        assert_eq!(
            ino(src.join("index.js")),
            ino(project.join("node_modules/a/index.js"))
        );
    }

    #[test]
    fn rejects_paths_escaping_node_modules() {
        let dir = tempfile::tempdir().unwrap();
        let src = cached(dir.path(), "a", &[]);
        for bad in ["node_modules/../x", "other/a", "/abs", "node_modules/a\\b"] {
            assert!(
                link(dir.path(), &[entry(bad, "a", src.clone())]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn replacing_a_parent_relinks_its_nested_packages() {
        let dir = tempfile::tempdir().unwrap();
        let a = cached(dir.path(), "a", &[("index.js", "1")]);
        let b = cached(dir.path(), "b", &[("index.js", "b")]);
        let project = dir.path().join("p");
        let nested = entry("node_modules/a/node_modules/b", "b", b);
        link(
            &project,
            &[entry("node_modules/a", "a", a.clone()), nested.clone()],
        )
        .unwrap();
        let mut a2 = entry("node_modules/a", "a", a);
        a2.integrity = "sha512-y".to_owned();
        let report = link(&project, &[a2, nested]).unwrap();
        assert!(
            project
                .join("node_modules/a/node_modules/b/index.js")
                .exists()
        );
        assert_eq!(report.added.len(), 2);
    }

    #[test]
    fn removed_packages_and_empty_scopes_disappear() {
        let dir = tempfile::tempdir().unwrap();
        let a = cached(dir.path(), "a", &[("index.js", "1")]);
        let project = dir.path().join("p");
        link(&project, &[entry("node_modules/@s/a", "@s/a", a)]).unwrap();
        let report = link(&project, &[]).unwrap();
        assert_eq!(report.removed, ["@s/a@1.0.0"]);
        assert!(!project.join("node_modules/@s").exists());
    }

    #[cfg(unix)]
    #[test]
    fn bin_symlinks_follow_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let a = cached(
            dir.path(),
            "a",
            &[
                (
                    "package.json",
                    r#"{"bin":{"tool":"./cli.js","evil":"../x"}}"#,
                ),
                ("cli.js", "#!/usr/bin/env node"),
            ],
        );
        let project = dir.path().join("p");
        link(&project, &[entry("node_modules/a", "a", a)]).unwrap();
        let tool = project.join("node_modules/.bin/tool");
        assert_eq!(std::fs::read_link(&tool).unwrap(), Path::new("../a/cli.js"));
        assert!(!project.join("node_modules/.bin/evil").exists());
        link(&project, &[]).unwrap();
        assert!(tool.symlink_metadata().is_err());
    }
}

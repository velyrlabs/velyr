//! The files a compiled executable embeds: the entry's module graph, the `--include` assets and the
//! `package.json` files that module resolution reads at run time.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use regex::Regex;
use velyr_runtime_core::ModuleResolverHandle;
use velyr_runtime_modules::{FsResolver, ModuleId, ResolveContext, is_embedded};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_runtime_vfs::edge_key;

/// Directory the entry's folder becomes inside the executable.
const VIRTUAL_ROOT: &str = "/$velyr/app";
/// Files outside the entry's folder (a parent `node_modules`, `../shared.ts`) live here, under the
/// root so the import permission that covers the root covers them too.
const EXTERNAL_DIR: &str = ".external";
const SCRIPT_EXTENSIONS: [&str; 8] = ["js", "mjs", "cjs", "ts", "mts", "cts", "jsx", "tsx"];

/// Matches the string of `from "x"`, `import "x"`, `import("x")`, `require("x")` and
/// `createRequire(url)("x")`.
// shortcut: a textual scan, so computed specifiers are not followed and a match inside a string or
// comment only costs a warning; upgrade by collecting the module requests from the engine.
static SPECIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:\b(?:from|import|require)\s*\(?|\bcreateRequire\s*\([^)]*\)\s*\()\s*["']([^"'\n]+)["']"#,
    )
    .expect("valid regex")
});

pub(super) fn scan_specifiers(source: &str) -> Vec<String> {
    SPECIFIER
        .captures_iter(source)
        .map(|c| c[1].to_owned())
        .collect()
}

pub(super) fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|e| format!("error: {}: {e}", path.display()))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("error: {}: {e}", path.display()))
}

pub(super) struct Graph {
    root: PathBuf,
    allow_outside_root: bool,
    resolver: ModuleResolverHandle,
    pub files: BTreeMap<String, Vec<u8>>,
    pub edges: BTreeMap<String, String>,
}

impl Graph {
    pub fn new(root: PathBuf, allow_outside_root: bool) -> Self {
        // The build reads every file it embeds, so the project wrapper may read exactly those.
        let mut builder = CapabilitySet::builder();
        for scope in grant_scopes(&root, allow_outside_root) {
            builder = builder.allow(Capability::ImportLocal(PathPattern::new(
                scope.to_string_lossy(),
            )));
        }
        let permissions = builder.build();
        let resolver =
            crate::transforms::project_resolver(crate::profiles::module_resolver(), &permissions)
                .unwrap_or_else(|| Arc::new(FsResolver));
        Self {
            root,
            allow_outside_root,
            resolver,
            files: BTreeMap::new(),
            edges: BTreeMap::new(),
        }
    }

    /// Files under the root, or under a `node_modules` of the root or one of its parents; anything
    /// else only with `--allow-outside-root`.
    fn is_embeddable(&self, path: &Path) -> bool {
        let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.allow_outside_root
            || grant_scopes(&self.root, false)
                .iter()
                .any(|scope| real.starts_with(scope))
    }

    fn check_embeddable(&self, path: &Path) -> Result<(), String> {
        if self.is_embeddable(path) {
            return Ok(());
        }
        Err(format!(
            "error: {} is outside the project root {}; move it under the root or pass --allow-outside-root",
            path.display(),
            self.root.display()
        ))
    }

    /// Where `path` lives inside the executable.
    pub fn virtual_path(&self, path: &Path) -> String {
        match path.strip_prefix(&self.root) {
            Ok(rel) => format!("{VIRTUAL_ROOT}/{}", rel.to_string_lossy()),
            Err(_) => format!("{VIRTUAL_ROOT}/{EXTERNAL_DIR}{}", path.to_string_lossy()),
        }
    }

    /// Embeds `entry` and every file reachable from it: `import` edges through the real resolver,
    /// `require` calls of CommonJS files through the CommonJS resolver.
    // shortcut: only the nearest package.json of each file is embedded, and a `node_modules`
    // outside the entry's folder is not found by `require` at run time (ESM imports are, through
    // the recorded edges); upgrade by embedding every manifest `require` could read.
    pub fn add_module(&mut self, entry: &Path) -> Result<(), String> {
        let mut queue = VecDeque::from([entry.to_path_buf()]);
        while let Some(path) = queue.pop_front() {
            let virtual_path = self.virtual_path(&path);
            if self.files.contains_key(&virtual_path) {
                continue;
            }
            self.check_embeddable(&path)?;
            if !path.starts_with(&self.root) && !is_in_node_modules(&path) {
                eprintln!(
                    "warning: embedding {} from outside the project root",
                    path.display()
                );
            }
            let bytes = read(&path)?;
            let scanned = match (is_script(&path), std::str::from_utf8(&bytes)) {
                (true, Ok(text)) => scan_specifiers(text),
                _ => Vec::new(),
            };
            self.files.insert(virtual_path.clone(), bytes);
            self.add_nearest_manifest(&path)?;
            let commonjs = is_commonjs(&path);
            let referrer =
                ModuleId::from_path(&path).ok_or("error: module path is not absolute")?;
            for specifier in scanned {
                if specifier.starts_with("node:") {
                    continue;
                }
                let found = if commonjs {
                    require(&path, &specifier)
                } else {
                    self.import(&virtual_path, &referrer, &specifier)
                };
                match found {
                    Ok(Some(target)) => queue.push_back(target),
                    Ok(None) => {}
                    Err(error) if specifier.starts_with(['.', '/']) => {
                        eprintln!(
                            "warning: {}: cannot resolve '{specifier}': {error}",
                            path.display()
                        );
                    }
                    // A builtin or an optional dependency: resolved, or not, again at run time.
                    Err(_) => {}
                }
            }
        }
        Ok(())
    }

    /// Resolves an `import`; the edge is recorded because the executable never resolves them.
    fn import(
        &mut self,
        from: &str,
        referrer: &ModuleId,
        specifier: &str,
    ) -> Result<Option<PathBuf>, String> {
        let found = self
            .resolver
            .resolve(specifier, referrer, &ResolveContext::default())
            .map_err(|e| e.to_string())?;
        // Embedded helpers are not files; the VFS fallback hands them to the wrapped resolver.
        if is_generated(&found.path) || is_embedded(&found.path) {
            return Ok(None);
        }
        let target = self.virtual_path(&found.path);
        self.edges.insert(edge_key(from, specifier), target);
        Ok(Some(found.path))
    }

    /// `detect` and `resolve` read it at run time: the module format and the `main`/`exports` of a
    /// package come from the nearest manifest.
    fn add_nearest_manifest(&mut self, module: &Path) -> Result<(), String> {
        for dir in module.ancestors().skip(1) {
            let manifest = dir.join("package.json");
            if manifest.is_file() {
                if !self.is_embeddable(&manifest) {
                    break;
                }
                let key = self.virtual_path(&manifest);
                if let Entry::Vacant(slot) = self.files.entry(key) {
                    slot.insert(read(&manifest)?);
                }
                break;
            }
        }
        Ok(())
    }

    pub fn add_assets(&mut self, pattern: &str) -> Result<(), String> {
        // A trailing `**` matches nothing in the glob crate; `dir/**/*` is the recursive spelling.
        let spelled = if pattern.ends_with("**") {
            format!("{pattern}/*")
        } else {
            pattern.to_owned()
        };
        let paths = glob::glob(&spelled).map_err(|e| format!("error: --include {pattern}: {e}"))?;
        let mut matched = false;
        // A directory stands for the files below it.
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        for path in paths.flatten() {
            collect_files(&path, &mut files, &mut seen);
        }
        for path in files {
            matched = true;
            let path = canonical(&path)?;
            let bytes = read(&path)?;
            self.files.entry(self.virtual_path(&path)).or_insert(bytes);
        }
        if !matched {
            eprintln!("warning: --include {pattern} matched no files");
        }
        Ok(())
    }
}

/// Each real directory is walked once, so a symlink back into an ancestor is skipped.
fn collect_files(path: &Path, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let Ok(real) = path.canonicalize() else {
        return;
    };
    if !seen.insert(real) {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            collect_files(&entry.path(), out, seen);
        }
    }
}

/// The root plus the `node_modules` of the root and of each parent directory.
fn grant_scopes(root: &Path, allow_outside_root: bool) -> Vec<PathBuf> {
    if allow_outside_root {
        return vec![root.ancestors().last().unwrap_or(root).to_path_buf()];
    }
    let parents = root.ancestors().map(|dir| dir.join("node_modules"));
    std::iter::once(root.to_path_buf()).chain(parents).collect()
}

fn is_in_node_modules(path: &Path) -> bool {
    path.components()
        .any(|c| c == std::path::Component::Normal("node_modules".as_ref()))
}

fn is_script(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SCRIPT_EXTENSIONS.contains(&e))
}

/// Without the node profile nothing runs as CommonJS.
fn is_commonjs(path: &Path) -> bool {
    #[cfg(feature = "node")]
    {
        use velyr_compat_commonjs::{ModuleFormat, detect_format};
        is_script(path) && detect_format(path) == ModuleFormat::CommonJs
    }
    #[cfg(not(feature = "node"))]
    {
        let _ = path;
        false
    }
}

/// The file `require(specifier)` from `from` loads, by the runtime's own rules.
fn require(from: &Path, specifier: &str) -> Result<Option<PathBuf>, String> {
    #[cfg(feature = "node")]
    {
        velyr_compat_commonjs::resolve_require(specifier, from)
            .map(Some)
            .map_err(|e| e.to_string())
    }
    #[cfg(not(feature = "node"))]
    {
        let _ = (from, specifier);
        Ok(None)
    }
}

/// The `node:` facades are generated outside the project; the executable regenerates them.
fn is_generated(path: &Path) -> bool {
    #[cfg(feature = "node")]
    if let Ok(dir) = crate::node::facade_dir() {
        return path.starts_with(dir);
    }
    let _ = path;
    false
}

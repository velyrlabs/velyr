//! Per-file `tsconfig.json` discovery for the TypeScript pipeline and the project resolver.
//!
//! Discovery walks upward from the importing file, never reads a directory the run is not
//! authorized for, and gives files under `node_modules/` no project config (D8).

mod config;
pub mod resolver;

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub use config::{CompilerOptions, Paths, TsConfig};
pub use resolver::{ImportMap, ProjectResolver};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::authorize_path;
use velyr_runtime_permissions::CapabilitySet;

use config::{RawConfig, normalize, parse_raw};

const CONFIG_NAME: &str = "tsconfig.json";
const MAX_EXTENDS_DEPTH: usize = 16;
const MAX_EXTENDS_FILES: usize = 64;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

type Cache = HashMap<PathBuf, Option<Arc<TsConfig>>>;

pub struct TsconfigLoader {
    cache: Mutex<Cache>,
    permissions: Arc<CapabilitySet>,
    reads: AtomicUsize,
}

impl TsconfigLoader {
    pub fn new(permissions: Arc<CapabilitySet>) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            permissions,
            reads: AtomicUsize::new(0),
        }
    }

    /// Number of config files read from disk so far (test hook for cache behavior).
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }

    /// Nearest `tsconfig.json` above `file`, fully merged. `Ok(None)` when there is none, when
    /// `file` is under `node_modules/`, or when the search leaves the permitted roots.
    pub fn for_file(&self, file: &Path) -> Result<Option<Arc<TsConfig>>, VelyrError> {
        if file
            .components()
            .any(|c| c == Component::Normal("node_modules".as_ref()))
        {
            return Ok(None);
        }
        let Some(dir) = file.parent() else {
            return Ok(None);
        };
        if let Some(hit) = self.cache.lock().unwrap().get(dir) {
            return Ok(hit.clone());
        }
        let found = self.discover(dir)?;
        self.cache
            .lock()
            .unwrap()
            .insert(dir.to_path_buf(), found.clone());
        Ok(found)
    }

    fn discover(&self, dir: &Path) -> Result<Option<Arc<TsConfig>>, VelyrError> {
        for ancestor in dir.ancestors() {
            let candidate = ancestor.join(CONFIG_NAME);
            if authorize_path(&candidate, &self.permissions).is_err() {
                return Ok(None);
            }
            if candidate.is_file() {
                return self.load_top(candidate).map(Some);
            }
        }
        Ok(None)
    }

    fn load_top(&self, path: PathBuf) -> Result<Arc<TsConfig>, VelyrError> {
        let mut options = self.load(&path, &mut LoadState::default())?;
        if let (Some(paths), Some(base_url)) = (options.paths.as_mut(), options.base_url.as_ref()) {
            paths.base = base_url.clone();
        }
        Ok(Arc::new(TsConfig { path, options }))
    }

    fn load(&self, path: &Path, state: &mut LoadState) -> Result<CompilerOptions, VelyrError> {
        if state.chain.len() >= MAX_EXTENDS_DEPTH {
            return Err(extends_error(
                path,
                "extends chain is deeper than 16 levels",
            ));
        }
        if state.chain.iter().any(|seen| seen == path) {
            return Err(extends_error(path, "extends cycle"));
        }
        if let Some(done) = state.done.get(path) {
            return Ok(done.clone());
        }
        if state.done.len() >= MAX_EXTENDS_FILES {
            return Err(extends_error(
                path,
                "extends pulls in more than 64 config files",
            ));
        }
        let text = read_capped(path)?;
        self.reads.fetch_add(1, Ordering::Relaxed);
        let mut raw: RawConfig = parse_raw(path, &text)?;
        let dir = path.parent().unwrap_or(Path::new("."));

        state.chain.push(path.to_path_buf());
        let mut inherited = CompilerOptions::default();
        for spec in raw.extends.take().map(|e| e.into_vec()).unwrap_or_default() {
            let parent = self.resolve_extends(dir, &spec, path)?;
            inherited = self.load(&parent, state)?.over(inherited);
        }
        state.chain.pop();
        let options = raw.options(dir).over(inherited);
        state.done.insert(path.to_path_buf(), options.clone());
        Ok(options)
    }

    /// The canonical path of the config `spec` names. Authorization and the later read use that
    /// same path, so a symlink followed by `..` cannot reach a file the grant does not cover.
    fn resolve_extends(&self, dir: &Path, spec: &str, from: &Path) -> Result<PathBuf, VelyrError> {
        let is_path =
            spec.starts_with("./") || spec.starts_with("../") || Path::new(spec).is_absolute();
        let roots: Vec<PathBuf> = if is_path {
            vec![normalize(&dir.join(spec))]
        } else if Path::new(spec)
            .components()
            .any(|c| c == Component::ParentDir)
        {
            Vec::new()
        } else {
            dir.ancestors()
                .map(|a| a.join("node_modules").join(spec))
                .collect()
        };
        roots
            .iter()
            .flat_map(|root| extends_candidates(root))
            .filter_map(|c| dunce::canonicalize(c).ok())
            .find(|c| authorize_path(c, &self.permissions).is_ok() && c.is_file())
            .ok_or_else(|| extends_error(from, &format!("cannot resolve extends `{spec}`")))
    }
}

#[derive(Default)]
struct LoadState {
    chain: Vec<PathBuf>,
    done: HashMap<PathBuf, CompilerOptions>,
}

fn read_capped(path: &Path) -> Result<String, VelyrError> {
    let io_error = |e: std::io::Error| VelyrError::invalid_data(format!("{}: {e}", path.display()));
    let mut text = String::new();
    fs::File::open(path)
        .map_err(io_error)?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(io_error)?;
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(extends_error(path, "config file is larger than 1 MiB"));
    }
    Ok(text)
}

fn extends_candidates(root: &Path) -> Vec<PathBuf> {
    let mut with_json = root.as_os_str().to_owned();
    with_json.push(".json");
    vec![root.to_path_buf(), with_json.into(), root.join(CONFIG_NAME)]
}

fn extends_error(path: &Path, reason: &str) -> VelyrError {
    VelyrError::invalid_data(format!("{}: {reason}", path.display()))
}

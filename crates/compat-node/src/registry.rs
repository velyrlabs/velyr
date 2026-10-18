//! The `node:` builtin registry: which modules exist, the JS bootstrap that defines them, the
//! ESM facade each one gets, and the resolver that maps `node:x` / `x` onto those facades.
//!
//! An ESM `import` of a builtin resolves to `/$velyr/node/<name>.js`, a facade the module host
//! serves from memory without a grant (see `runtime_modules::embedded`); it just re-exports the
//! isolate's builtin. Windows keeps generated files in a directory the host grants.
//! The export lists and factory sources are data and live in `registry_tables.rs`.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{
    ModuleHost, ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result,
};

use crate::registry_tables::FACTORIES;
pub use crate::registry_tables::{BUILTINS, Builtin};

const REGISTRY_JS: &str = include_str!("registry.js");

/// The script that defines every builtin lazily and publishes them on
/// `globalThis[Symbol.for("velyr.node")]` (and into `require`'s builtin table, if present).
pub fn bootstrap_source() -> String {
    let mut factories = String::new();
    let all = FACTORIES
        .iter()
        .chain(crate::streams::STREAM_FACTORIES)
        .chain(crate::crypto_js::CRYPTO_FACTORIES)
        .chain(crate::fs_js::FS_FACTORIES)
        .chain(crate::registry_net::NET_FACTORIES)
        .chain(crate::child_process_ops::CHILD_FACTORIES)
        .chain(crate::worker_threads_ops::WORKER_FACTORIES)
        .chain(crate::registry_http::HTTP_FACTORIES)
        .chain(crate::registry_test::TEST_FACTORIES)
        .chain(crate::registry_async::ASYNC_FACTORIES)
        .chain(crate::registry_extra::EXTRA_FACTORIES);
    for (name, body) in all {
        factories.push_str(&format!(
            "{name:?}: function (module, exports, require) {{\n{body}\n}},\n"
        ));
    }
    let public: Vec<String> = BUILTINS.iter().map(|b| format!("{:?}", b.name)).collect();
    REGISTRY_JS
        .replace("/*FACTORIES*/", &factories)
        .replace("/*PUBLIC*/", &public.join(", "))
}

/// `Some(canonical name)` when `specifier` names a builtin, with or without `node:`.
pub fn builtin_name(specifier: &str) -> Option<&'static str> {
    let bare = specifier.strip_prefix("node:").unwrap_or(specifier);
    // `test` only exists as `node:test`: a bare `test` is an npm package.
    if bare == "test" && bare == specifier {
        return None;
    }
    BUILTINS.iter().find(|b| b.name == bare).map(|b| b.name)
}

#[cfg(windows)]
fn facade_file(name: &str) -> String {
    format!("{}.mjs", name.replace('/', "__"))
}

/// `facade_source` by name, the shape `ModuleHost::register_embedded_facades` wants.
fn facade_source_of(name: &str) -> Option<String> {
    BUILTINS.iter().find(|b| b.name == name).map(facade_source)
}

pub fn facade_source(builtin: &Builtin) -> String {
    let mut code = format!(
        "const m = globalThis[Symbol.for(\"velyr.node\")].require({:?});\nexport default m;\n",
        builtin.name
    );
    for export in builtin.exports {
        code.push_str(&format!("export const {export} = m.{export};\n"));
    }
    code
}

/// Atomically (write + rename) replaces `dir/file` unless it already holds exactly `source`.
/// Content is always compared: the directory is trusted via `ImportLocal`, so no hash/marker shortcut.
#[cfg(windows)]
fn write_if_changed(dir: &Path, file: &str, source: &str) -> io::Result<()> {
    let target = dir.join(file);
    if std::fs::read(&target).is_ok_and(|current| current == source.as_bytes()) {
        return Ok(());
    }
    let staging = dir.join(format!(".{file}.{}", std::process::id()));
    std::fs::write(&staging, source)?;
    std::fs::rename(&staging, &target)
}

/// Resolves `node:x` and bare `x` builtins to facade modules; everything else goes to `inner`.
#[derive(Clone)]
pub struct NodeBuiltinResolver {
    inner: Arc<dyn ModuleResolver + Send + Sync>,
    dir: PathBuf,
}

impl NodeBuiltinResolver {
    /// Registers the in-memory facades; nothing is written to `dir` (kept for tools that want real
    /// files, and for Windows, which still generates them there: the host must grant `ImportLocal`
    /// on [`facade_dir`](Self::facade_dir) and nothing wider).
    pub fn new(inner: Arc<dyn ModuleResolver + Send + Sync>, dir: &Path) -> io::Result<Self> {
        static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
        let names = NAMES.get_or_init(|| BUILTINS.iter().map(|b| b.name).collect());
        ModuleHost::register_embedded_facades(names, facade_source_of);
        #[cfg(windows)]
        let dir = {
            let context = |what: &str, path: &Path| {
                let path = path.display().to_string();
                let what = what.to_owned();
                move |err: io::Error| io::Error::new(err.kind(), format!("{what} {path}: {err}"))
            };
            std::fs::create_dir_all(dir).map_err(context("create", dir))?;
            let dir = std::fs::canonicalize(dir).map_err(context("canonicalize", dir))?;
            for builtin in BUILTINS {
                let file = facade_file(builtin.name);
                write_if_changed(&dir, &file, &facade_source(builtin))
                    .map_err(context("write", &dir.join(&file)))?;
            }
            dir
        };
        #[cfg(not(windows))]
        let dir = dir.to_path_buf();
        Ok(Self { inner, dir })
    }

    pub fn facade_dir(&self) -> &Path {
        &self.dir
    }
}

impl ModuleResolver for NodeBuiltinResolver {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleId,
        ctx: &ResolveContext,
    ) -> Result<ResolvedModule> {
        let Some(name) = builtin_name(specifier) else {
            // Node compat is linked here: an unknown `node:x` is a missing builtin, not a missing link.
            if specifier.starts_with("node:") {
                return Err(VelyrError::unsupported(format!(
                    "builtin {specifier} is not supported yet"
                )));
            }
            return self.inner.resolve(specifier, referrer, ctx);
        };
        #[cfg(windows)]
        let path = self.dir.join(facade_file(name));
        #[cfg(not(windows))]
        let path = PathBuf::from(format!("/$velyr/node/{name}.js"));
        let id = ModuleId::from_path(&path)
            .ok_or_else(|| VelyrError::invalid_data("facade path is not representable as a URL"))?;
        Ok(ResolvedModule { id, path })
    }
}

//! The `node:` builtin registry: which modules exist, the JS bootstrap that defines them, the
//! ESM facade each one gets, and the resolver that maps `node:x` / `x` onto those facades.
//!
//! The module host reads every module from a file and authorizes its path, so an ESM `import` of
//! a builtin resolves to a generated facade file inside a directory the host chooses and grants
//! (`ImportLocal` on that directory only). The facade just re-exports the isolate's builtin.
//! The export lists and factory sources are data and live in `registry_tables.rs`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result};

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

fn facade_file(name: &str) -> String {
    format!("{}.mjs", name.replace('/', "__"))
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

/// Resolves `node:x` and bare `x` builtins to facade modules; everything else goes to `inner`.
#[derive(Clone)]
pub struct NodeBuiltinResolver {
    inner: Arc<dyn ModuleResolver + Send + Sync>,
    dir: PathBuf,
}

impl NodeBuiltinResolver {
    /// Writes the facade files into `dir` (created if needed; existing facades are overwritten).
    /// The host must grant `ImportLocal` on [`facade_dir`](Self::facade_dir) and nothing wider,
    /// and `dir` must not be writable by other users.
    pub fn new(inner: Arc<dyn ModuleResolver + Send + Sync>, dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let dir = fs::canonicalize(dir)?;
        for builtin in BUILTINS {
            let target = dir.join(facade_file(builtin.name));
            let staging = dir.join(format!(
                ".{}.{}",
                facade_file(builtin.name),
                std::process::id()
            ));
            fs::write(&staging, facade_source(builtin))?;
            fs::rename(&staging, &target)?;
        }
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
        let path = self.dir.join(facade_file(name));
        let id = ModuleId::from_path(&path)
            .ok_or_else(|| VelyrError::invalid_data("facade path is not representable as a URL"))?;
        Ok(ResolvedModule { id, path })
    }
}

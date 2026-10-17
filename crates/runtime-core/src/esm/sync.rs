//! Synchronous module loading for `require(esm)`: resolve, read, link and evaluate a graph from
//! inside a host function. It shares the source cache of the asynchronous `import` driver, and
//! the engine keys modules by name, so a module loaded by either side has a single instance.

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::{Arc, Mutex};

use velyr_engine_api::{
    CallContext, HostError, Isolate, ModuleResolveHook, PromiseState, Source, Value,
};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{
    FsResolver, MediaType, ModuleId, ModuleSource, ResolveContext, ResolvedModule, SourceTransform,
    authorize_path,
};
use velyr_runtime_permissions::CapabilitySet;

use crate::ModuleResolverHandle;

/// Sources read so far, by module identity. Shared by the `import` driver and [`EsmRequire`].
pub(super) type SourceCache = Arc<Mutex<HashMap<ModuleId, Arc<ModuleSource>>>>;

/// Isolate slot behind `require(esm)`. Install it with [`EsmRequire::install`]; the runtime does
/// it for the entry module's isolate. Hosts embedding a `require` call [`require_esm`].
#[derive(Clone)]
pub struct EsmRequire {
    resolver: ModuleResolverHandle,
    resolve_ctx: ResolveContext,
    permissions: Arc<CapabilitySet>,
    transform: Option<Arc<dyn SourceTransform>>,
    loaded: SourceCache,
}

impl EsmRequire {
    /// `permissions` gate every file read (`ImportLocal` or `FsRead`), as for `import`.
    pub fn new(resolver: Option<ModuleResolverHandle>, permissions: Arc<CapabilitySet>) -> Self {
        Self {
            resolver: resolver.unwrap_or_else(|| Arc::new(FsResolver)),
            resolve_ctx: ResolveContext::default(),
            permissions,
            transform: None,
            loaded: SourceCache::default(),
        }
    }

    /// Runs on every module read here, like `ModuleHost::with_transform` does for `import`.
    #[must_use]
    pub fn with_transform(mut self, transform: Arc<dyn SourceTransform>) -> Self {
        self.transform = Some(transform);
        self
    }

    pub fn install(self, isolate: &mut impl Isolate) {
        isolate.set_slot(self);
    }

    pub(super) fn sharing(mut self, loaded: SourceCache) -> Self {
        self.loaded = loaded;
        self
    }

    fn read(&self, module: &ResolvedModule) -> Result<Arc<ModuleSource>, VelyrError> {
        authorize_path(&module.path, &self.permissions)?;
        if let Some(hit) = self.loaded.lock().unwrap().get(&module.id) {
            return Ok(Arc::clone(hit));
        }
        let text = match velyr_runtime_vfs::covering(&module.path) {
            // A compiled executable's embedded modules are not on disk.
            Some(vfs) => vfs
                .read(&module.path.to_string_lossy())
                .ok_or_else(|| VelyrError::not_found("module not found"))
                .and_then(|bytes| {
                    std::str::from_utf8(bytes)
                        .map(str::to_owned)
                        .map_err(|_| VelyrError::invalid_data("module is not readable UTF-8 text"))
                })?,
            None => fs::read_to_string(&module.path).map_err(|err| match err.kind() {
                ErrorKind::NotFound => VelyrError::not_found("module not found"),
                _ => VelyrError::invalid_data("module is not readable UTF-8 text").with_cause(err),
            })?,
        };
        let text = match &self.transform {
            // shortcut: source maps of modules read here are dropped; upgrade with a shared map store.
            Some(transform) => {
                let path = module.path.to_string_lossy();
                transform
                    .transform(&path, &text, MediaType::from_path(&module.path))?
                    .code
            }
            None => text,
        };
        let source = Arc::new(ModuleSource::new(text));
        self.loaded
            .lock()
            .unwrap()
            .insert(module.id.clone(), Arc::clone(&source));
        Ok(source)
    }

    /// Loads, links and evaluates the module at the absolute `path`. `Some(namespace)` once
    /// evaluation finished synchronously; `None` when it did not (top-level await), which the
    /// caller reports as `ERR_REQUIRE_ASYNC_MODULE`. Errors thrown by the module are rethrown.
    pub fn require(
        &self,
        ctx: &mut dyn CallContext,
        path: &Path,
    ) -> Result<Option<Value>, HostError> {
        let id = ModuleId::from_path(path)
            .ok_or_else(|| VelyrError::invalid_data("module path is not absolute"))?;
        let root = self
            .resolver
            .resolve(&path.to_string_lossy(), &id, &self.resolve_ctx)?;
        let source = self.read(&root)?;
        let module = ctx.compile_module(&Source {
            name: root.id.as_str().to_owned(),
            text: source.text.clone(),
        })?;
        let mut hook = SyncHook {
            loader: self,
            failure: None,
        };
        if let Err(err) = ctx.instantiate_module(module, &mut hook) {
            return Err(hook.failure.map_or(err, HostError::Velyr));
        }
        let promise = ctx.evaluate_module(module)?;
        match ctx.promise_state(&promise) {
            Some(PromiseState::Fulfilled(_)) => ctx.module_namespace(module).map(Some),
            Some(PromiseState::Rejected(reason)) => Err(HostError::Thrown(reason)),
            _ => Ok(None),
        }
    }
}

/// [`EsmRequire::require`] through the slot the runtime (or the host) installed.
pub fn require_esm(ctx: &mut dyn CallContext, path: &Path) -> Result<Option<Value>, HostError> {
    let loader = ctx
        .isolate_slot::<EsmRequire>()
        .cloned()
        .ok_or_else(|| HostError::TypeError("require(esm) needs an installed EsmRequire".into()))?;
    loader.require(ctx, path)
}

struct SyncHook<'a> {
    loader: &'a EsmRequire,
    failure: Option<VelyrError>,
}

impl SyncHook<'_> {
    fn load(&self, specifier: &str, referrer: &str) -> Result<Source, VelyrError> {
        let referrer = ModuleId::parse(referrer).ok_or_else(|| {
            VelyrError::invalid_data(format!("referrer is not a module URL: {referrer}"))
        })?;
        let module =
            self.loader
                .resolver
                .resolve(specifier, &referrer, &self.loader.resolve_ctx)?;
        let source = self.loader.read(&module)?;
        Ok(Source {
            name: module.id.as_str().to_owned(),
            text: source.text.clone(),
        })
    }
}

impl ModuleResolveHook for SyncHook<'_> {
    fn resolve(&mut self, specifier: &str, referrer: &str) -> Result<Source, HostError> {
        self.load(specifier, referrer).map_err(|err| {
            self.failure = Some(err);
            HostError::Velyr(VelyrError::internal("module resolution failed"))
        })
    }
}

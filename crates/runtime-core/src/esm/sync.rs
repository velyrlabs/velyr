//! Synchronous module loading for `require(esm)`: resolve, read, link and evaluate a graph from
//! inside a host function. It shares the source cache of the asynchronous `import` driver, and
//! the engine keys modules by name, so a module loaded by either side has a single instance.

use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::io::ErrorKind;
use std::path::Path;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use velyr_engine_api::{
    CallContext, HostError, Isolate, ModuleResolveHook, PromiseState, SharedSource, Source, Value,
};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{
    FsResolver, LoadContext, MediaType, ModuleHost, ModuleId, ModuleSource, ResolveContext,
    ResolvedModule, SharedCache, SourceTransform, authorize_path,
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
    host: Option<Arc<ModuleHost>>,
    shared: Option<Arc<SharedCache>>,
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
            host: None,
            shared: None,
            loaded: SourceCache::default(),
        }
    }

    /// Runs on every module read here, like `ModuleHost::with_transform` does for `import`.
    #[must_use]
    pub fn with_transform(mut self, transform: Arc<dyn SourceTransform>) -> Self {
        self.transform = Some(transform);
        self
    }

    /// Reads through `host`, the module host `import` uses: one permission check, one source
    /// layer, source maps registered. Without it, files are read directly.
    #[must_use]
    pub fn with_host(mut self, host: Arc<ModuleHost>) -> Self {
        self.host = Some(host);
        self
    }

    /// Lets the dependencies of a required graph consume code caches published by `import`.
    #[must_use]
    pub fn with_shared(mut self, shared: Arc<SharedCache>) -> Self {
        self.shared = Some(shared);
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
        if let Some(host) = &self.host {
            return self.read_through(host, module);
        }
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

    fn read_through(
        &self,
        host: &ModuleHost,
        module: &ResolvedModule,
    ) -> Result<Arc<ModuleSource>, VelyrError> {
        // Entries of `loaded` came from `ModuleHost::load` under the same permissions.
        if let Some(hit) = self.loaded.lock().unwrap().get(&module.id) {
            return Ok(Arc::clone(hit));
        }
        let ctx = LoadContext {
            permissions: Arc::clone(&self.permissions),
        };
        let source = block_on(host.load(module, &ctx))?;
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
            text: source.text.to_string(),
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
    fn load(&self, specifier: &str, referrer: &str) -> Result<SharedSource, VelyrError> {
        let referrer = ModuleId::parse(referrer).ok_or_else(|| {
            VelyrError::invalid_data(format!("referrer is not a module URL: {referrer}"))
        })?;
        let module =
            self.loader
                .resolver
                .resolve(specifier, &referrer, &self.loader.resolve_ctx)?;
        let source = self.loader.read(&module)?;
        Ok(SharedSource {
            name: module.id.as_str().to_owned(),
            text: Arc::clone(&source.text),
        })
    }
}

impl ModuleResolveHook for SyncHook<'_> {
    fn code_cache(&mut self, name: &str) -> Option<Arc<[u8]>> {
        let shared = self.loader.shared.as_ref()?;
        let id = ModuleId::parse(name)?;
        let hash = self.loader.loaded.lock().unwrap().get(&id)?.hash.clone();
        shared.code(&id, &hash)
    }

    fn resolve(&mut self, specifier: &str, referrer: &str) -> Result<Source, HostError> {
        self.resolve_shared(specifier, referrer)
            .map(|source| source.to_source())
    }

    fn resolve_shared(
        &mut self,
        specifier: &str,
        referrer: &str,
    ) -> Result<SharedSource, HostError> {
        self.load(specifier, referrer).map_err(|err| {
            self.failure = Some(err);
            HostError::Velyr(VelyrError::internal("module resolution failed"))
        })
    }
}

struct Unparker(Thread);

impl Wake for Unparker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Drives `future` to completion on the calling thread. The isolate thread runs inside the Tokio
/// runtime, so `ModuleHost::load` finds its blocking pool and the pool thread unparks us.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unparker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        thread::park();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use velyr_runtime_modules::{BoxFuture, ModuleLoader, Transformed};
    use velyr_runtime_permissions::{Capability, PathPattern};

    use super::*;

    const MAP: &str = r#"{"version":3,"sources":["x.ts"],"names":[],"mappings":";AAAA"}"#;

    struct MapTransform;

    impl SourceTransform for MapTransform {
        fn transform(
            &self,
            _specifier: &str,
            source: &str,
            _media_type: MediaType,
        ) -> velyr_runtime_modules::Result<Transformed> {
            Ok(Transformed {
                code: source.to_owned(),
                source_map: Some(MAP.to_owned()),
            })
        }
    }

    #[derive(Default)]
    struct CountingLoader(AtomicUsize);

    impl ModuleLoader for CountingLoader {
        fn load<'a>(
            &'a self,
            _module: &'a ResolvedModule,
            _ctx: &'a LoadContext,
        ) -> BoxFuture<'a, velyr_runtime_modules::Result<ModuleSource>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(ModuleSource::new(String::new())) })
        }
    }

    fn module(name: &str, text: &str) -> (std::path::PathBuf, ResolvedModule) {
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("velyr-sync-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        let id = ModuleId::from_path(&path).unwrap();
        (dir, ResolvedModule { id, path })
    }

    fn allow(dir: &Path) -> Arc<CapabilitySet> {
        CapabilitySet::builder()
            .allow(Capability::ImportLocal(PathPattern::new(
                dir.to_str().unwrap(),
            )))
            .build()
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn read_through_host_registers_source_maps() {
        let (dir, module) = module("x.ts", "export const x = 1;");
        let host = Arc::new(
            ModuleHost::new(Arc::new(velyr_runtime_modules::FsLoader))
                .with_transform(Arc::new(MapTransform)),
        );
        let require = EsmRequire::new(None, allow(&dir)).with_host(Arc::clone(&host));
        let runtime = runtime();
        let _guard = runtime.enter();
        require.read(&module).unwrap();
        let mapper = host.source_maps().mapper();
        assert!(mapper(module.id.as_str(), 2, 1).is_some());
    }

    #[test]
    fn read_through_host_denies_an_ungranted_path_before_the_loader_runs() {
        let (_dir, module) = module("denied.mjs", "export {};");
        let loader = Arc::new(CountingLoader::default());
        let host = Arc::new(ModuleHost::new(loader.clone()));
        let other = std::env::temp_dir().join("velyr-sync-elsewhere");
        let require = EsmRequire::new(None, allow(&other)).with_host(host);
        let runtime = runtime();
        let _guard = runtime.enter();
        assert!(require.read(&module).is_err());
        assert_eq!(loader.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn read_through_host_keeps_the_module_hash() {
        let (dir, module) = module("same.mjs", "export const a = 1;");
        let direct = EsmRequire::new(None, allow(&dir)).read(&module).unwrap();
        let host = Arc::new(ModuleHost::new(Arc::new(velyr_runtime_modules::FsLoader)));
        let hosted = EsmRequire::new(None, allow(&dir)).with_host(host);
        let runtime = runtime();
        let _guard = runtime.enter();
        assert_eq!(hosted.read(&module).unwrap().hash, direct.hash);
    }
}

use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::{Arc, Mutex};

use velyr_runtime_errors::VelyrError;
use velyr_runtime_permissions::{CapabilitySet, Request};

use crate::{
    BoxFuture, LoadContext, MediaType, ModuleId, ModuleLoader, ModuleSource, ResolvedModule,
    Result, SharedCache, SourceMaps, SourceTransform,
};

/// Reads modules from disk. Opens only the canonical path of the resolved module.
#[derive(Debug, Default, Clone, Copy)]
pub struct FsLoader;

impl ModuleLoader for FsLoader {
    fn load<'a>(
        &'a self,
        module: &'a ResolvedModule,
        _ctx: &'a LoadContext,
    ) -> BoxFuture<'a, Result<ModuleSource>> {
        Box::pin(async move {
            let text =
                tokio::fs::read_to_string(&module.path)
                    .await
                    .map_err(|e| match e.kind() {
                        ErrorKind::NotFound => VelyrError::not_found("module not found"),
                        _ => VelyrError::invalid_data("module is not readable UTF-8 text")
                            .with_cause(e),
                    })?;
            Ok(ModuleSource::new(text))
        })
    }
}

/// In-memory modules keyed by ID, for tests and embedders.
#[derive(Debug, Default, Clone)]
pub struct VirtualLoader {
    modules: HashMap<ModuleId, String>,
}

impl VirtualLoader {
    pub fn with(mut self, id: ModuleId, text: impl Into<String>) -> Self {
        self.modules.insert(id, text.into());
        self
    }
}

impl ModuleLoader for VirtualLoader {
    fn load<'a>(
        &'a self,
        module: &'a ResolvedModule,
        _ctx: &'a LoadContext,
    ) -> BoxFuture<'a, Result<ModuleSource>> {
        Box::pin(async move {
            let text = self.modules.get(&module.id).ok_or_else(|| {
                VelyrError::not_found(format!("virtual module not found: {}", module.id.as_str()))
            })?;
            Ok(ModuleSource::new(text.clone()))
        })
    }
}

/// Authorizes, then loads through an injected loader, caching sources by module ID.
pub struct ModuleHost {
    loader: Arc<dyn ModuleLoader>,
    transform: Option<Arc<dyn SourceTransform>>,
    cache: Mutex<HashMap<ModuleId, Arc<ModuleSource>>>,
    transform_maps: Mutex<HashMap<ModuleId, String>>,
    source_maps: SourceMaps,
    shared: Option<Arc<SharedCache>>,
}

impl ModuleHost {
    pub fn new(loader: Arc<dyn ModuleLoader>) -> Self {
        Self {
            loader,
            transform: None,
            cache: Mutex::default(),
            transform_maps: Mutex::default(),
            source_maps: SourceMaps::default(),
            shared: None,
        }
    }

    /// Reads through the process-wide layer: a module another isolate already read is not read
    /// again. Authorization still runs first.
    pub fn with_shared(mut self, shared: Arc<SharedCache>) -> Self {
        self.shared = Some(shared);
        self
    }

    pub fn shared(&self) -> Option<&Arc<SharedCache>> {
        self.shared.as_ref()
    }

    /// Serves `/$velyr/node/<name>.js` from memory for exactly the given `names`, never from disk
    /// and without a grant. Process-wide and first call wins.
    pub fn register_embedded_facades(
        names: &'static [&'static str],
        source: fn(&str) -> Option<String>,
    ) {
        crate::embedded::register_facades(names, source);
    }

    /// Runs `transform` on every module after it is read and authorized; the cached and returned
    /// text (and its hash) is the transformed one.
    pub fn with_transform(mut self, transform: Arc<dyn SourceTransform>) -> Self {
        self.transform = Some(transform);
        self
    }

    /// Source map JSON of a loaded module, when its transform produced one.
    pub fn source_map(&self, id: &ModuleId) -> Option<String> {
        self.transform_maps.lock().unwrap().get(id).cloned()
    }

    /// Maps by the `file://` URL V8 reports in stack frames; feeds `Isolate::set_prepare_stack_trace`.
    pub fn source_maps(&self) -> &SourceMaps {
        &self.source_maps
    }

    /// `ImportLocal` or `FsRead` must cover the canonical path before anything is read.
    pub async fn load(
        &self,
        module: &ResolvedModule,
        ctx: &LoadContext,
    ) -> Result<Arc<ModuleSource>> {
        if let Some(text) = crate::embedded::source(&module.path) {
            return Ok(Arc::new(ModuleSource::new(text.into_owned())));
        }
        authorize(module, ctx)?;
        if let Some(hit) = self.cache.lock().unwrap().get(&module.id) {
            return Ok(hit.clone());
        }
        let mut source = match self
            .shared
            .as_ref()
            .and_then(|shared| shared.source(&module.id))
        {
            Some(raw) => ModuleSource::clone(&raw),
            None => {
                let raw = self.loader.load(module, ctx).await?;
                match &self.shared {
                    Some(shared) => ModuleSource::clone(&shared.insert_source(&module.id, raw)),
                    None => raw,
                }
            }
        };
        if let Some(transform) = &self.transform {
            source = self.apply(transform.clone(), module, source).await?;
        }
        self.source_maps.register(
            module,
            &source.text,
            self.source_map(&module.id),
            &ctx.permissions,
        );
        let source = Arc::new(source);
        self.cache
            .lock()
            .unwrap()
            .insert(module.id.clone(), source.clone());
        Ok(source)
    }
}

impl ModuleHost {
    async fn apply(
        &self,
        transform: Arc<dyn SourceTransform>,
        module: &ResolvedModule,
        source: ModuleSource,
    ) -> Result<ModuleSource> {
        let path = module.path.clone();
        let media_type = MediaType::from_path(&path);
        if !transform.handles(media_type) || !transform.applies(&module.id, &source.text) {
            return Ok(source);
        }
        let done = tokio::task::spawn_blocking(move || {
            transform.transform(&path.to_string_lossy(), &source.text, media_type)
        })
        .await
        .map_err(|e| VelyrError::internal("module transform did not finish").with_cause(e))??;
        if let Some(map) = done.source_map {
            self.transform_maps
                .lock()
                .unwrap()
                .insert(module.id.clone(), map);
        }
        Ok(ModuleSource::new(done.code))
    }
}

fn authorize(module: &ResolvedModule, ctx: &LoadContext) -> Result<()> {
    authorize_path(&module.path, &ctx.permissions)
}

/// `ImportLocal` or `FsRead` must cover `path`; the check `ModuleHost::load` makes, for hosts that
/// read module files synchronously (the CommonJS `require`).
pub fn authorize_path(path: &std::path::Path, permissions: &CapabilitySet) -> Result<()> {
    let denied = || VelyrError::permission_denied("import-local", "module");
    let path = path.to_str().ok_or_else(denied)?;
    let allowed = permissions.check(&Request::ImportLocal(path)).is_allow()
        || permissions.check(&Request::FsRead(path)).is_allow();
    if allowed { Ok(()) } else { Err(denied()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transformed;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use velyr_runtime_errors::ErrorCode;
    use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

    use crate::{FsResolver, ModuleResolver, ResolveContext};

    struct Spy<L>(L, Arc<AtomicUsize>);

    impl<L: ModuleLoader> ModuleLoader for Spy<L> {
        fn load<'a>(
            &'a self,
            module: &'a ResolvedModule,
            ctx: &'a LoadContext,
        ) -> BoxFuture<'a, Result<ModuleSource>> {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.load(module, ctx)
        }
    }

    fn ctx(caps: &[Capability]) -> LoadContext {
        let mut builder = CapabilitySet::builder();
        for cap in caps {
            builder = builder.allow(cap.clone());
        }
        LoadContext {
            permissions: builder.build(),
        }
    }

    fn grant(kind: fn(PathPattern) -> Capability, dir: &std::path::Path) -> Capability {
        kind(PathPattern::new(dir.to_str().unwrap()))
    }

    /// Two sibling dirs: `root/app/main.js` and `root/outside/secret.js`.
    fn tree() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for (d, f) in [("app", "main.js"), ("outside", "secret.js")] {
            fs::create_dir_all(root.join(d)).unwrap();
            fs::write(root.join(d).join(f), format!("// {f}")).unwrap();
        }
        (dir, root)
    }

    fn resolve(root: &std::path::Path, spec: &str) -> ResolvedModule {
        let referrer = ModuleId::from_path(&root.join("app/main.js")).unwrap();
        FsResolver
            .resolve(spec, &referrer, &ResolveContext::default())
            .unwrap()
    }

    fn spy_host(calls: &Arc<AtomicUsize>) -> ModuleHost {
        ModuleHost::new(Arc::new(Spy(FsLoader, calls.clone())))
    }

    #[tokio::test]
    async fn outside_root_without_fs_read_is_denied_before_loading() {
        let (_d, root) = tree();
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx(&[grant(Capability::ImportLocal, &root.join("app"))]);
        let err = spy_host(&calls)
            .load(&resolve(&root, "../outside/secret.js"), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::PermissionDenied);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn import_local_inside_root_loads() {
        let (_d, root) = tree();
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx(&[grant(Capability::ImportLocal, &root.join("app"))]);
        let src = spy_host(&calls)
            .load(&resolve(&root, "./main.js"), &ctx)
            .await
            .unwrap();
        assert_eq!(&*src.text, "// main.js");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fs_read_outside_root_allows_import() {
        let (_d, root) = tree();
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx(&[
            grant(Capability::ImportLocal, &root.join("app")),
            grant(Capability::FsRead, &root.join("outside")),
        ]);
        let src = spy_host(&calls)
            .load(&resolve(&root, "../outside/secret.js"), &ctx)
            .await
            .unwrap();
        assert_eq!(&*src.text, "// secret.js");
    }

    #[test]
    fn import_local_never_grants_fs_read() {
        let (_d, root) = tree();
        let ctx = ctx(&[grant(Capability::ImportLocal, &root)]);
        let path = root.join("app/main.js");
        let request = Request::FsRead(path.to_str().unwrap());
        assert!(!ctx.permissions.check(&request).is_allow());
    }

    #[tokio::test]
    async fn cache_returns_same_source_and_hash_without_reloading() {
        let (_d, root) = tree();
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx(&[grant(Capability::ImportLocal, &root)]);
        let host = spy_host(&calls);
        let module = resolve(&root, "./main.js");
        let first = host.load(&module, &ctx).await.unwrap();
        let second = host.load(&module, &ctx).await.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.hash, ModuleSource::new("// main.js".into()).hash);
        assert_eq!(first.hash.len(), 64);
    }

    #[tokio::test]
    async fn cache_hit_still_requires_permission() {
        let (_d, root) = tree();
        let calls = Arc::new(AtomicUsize::new(0));
        let host = spy_host(&calls);
        let module = resolve(&root, "./main.js");
        host.load(&module, &ctx(&[grant(Capability::ImportLocal, &root)]))
            .await
            .unwrap();
        let err = host.load(&module, &ctx(&[])).await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::PermissionDenied);
    }

    #[tokio::test]
    async fn virtual_loader_serves_injected_source() {
        let (_d, root) = tree();
        let module = resolve(&root, "./main.js");
        let loader = VirtualLoader::default().with(module.id.clone(), "export {}");
        let host = ModuleHost::new(Arc::new(loader));
        let src = host
            .load(&module, &ctx(&[grant(Capability::ImportLocal, &root)]))
            .await
            .unwrap();
        assert_eq!(&*src.text, "export {}");
    }

    struct Counting(Arc<AtomicUsize>);

    impl SourceTransform for Counting {
        fn handles(&self, media_type: MediaType) -> bool {
            media_type.is_typescript()
        }
        fn transform(&self, _: &str, source: &str, _: MediaType) -> Result<Transformed> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Transformed {
                code: format!("{source}//ts"),
                source_map: None,
            })
        }
    }

    #[tokio::test]
    async fn transform_is_skipped_for_media_it_does_not_handle() {
        let (_d, root) = tree();
        fs::write(root.join("app/typed.ts"), "// typed").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let host =
            ModuleHost::new(Arc::new(FsLoader)).with_transform(Arc::new(Counting(calls.clone())));
        let ctx = ctx(&[grant(Capability::ImportLocal, &root)]);
        let js = host.load(&resolve(&root, "./main.js"), &ctx).await.unwrap();
        assert_eq!(&*js.text, "// main.js");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let ts = host
            .load(&resolve(&root, "./typed.ts"), &ctx)
            .await
            .unwrap();
        assert_eq!(&*ts.text, "// typed//ts");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

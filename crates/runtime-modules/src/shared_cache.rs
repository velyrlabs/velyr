//! Process-wide immutable cache layer shared by the module hosts of every isolate (VELYR-60,
//! experimental, behind `--experimental-shared-code-cache`; decision pending in ADR-016).
//!
//! Only immutable layers are shared (docs/architecture/module-system.md): raw loader output by
//! module ID and sealed engine code caches by (module ID, content hash). Entries are set once and
//! never replaced. Authorization always runs before a lookup, and the transform still runs per
//! isolate (its output may depend on that isolate's permissions), so only the code cache, keyed by
//! the transformed text's hash, is shared past it.
//!
//! Source entries remember the file's `(mtime, len)`; a lookup stats the file and treats a
//! divergent stamp as a miss, so the next read replaces the entry.
//!
//! shortcut: the stamp is taken after the read, so an edit in between can pair new stamp with old
//! text; upgrade by stamping before the read if that ever matters.

use std::collections::HashMap;
use std::time::SystemTime;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use crate::{ModuleId, ModuleSource};

/// Sealed code cache by (module ID, content hash).
type CodeEntries = HashMap<(ModuleId, String), Arc<[u8]>>;

/// What a source file looked like when it was read; `None` for modules with no file behind them.
type Stamp = Option<(SystemTime, u64)>;

fn stamp(id: &ModuleId) -> Stamp {
    let meta = std::fs::metadata(id.to_path()?).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

static GLOBAL: OnceLock<Arc<SharedCache>> = OnceLock::new();

#[derive(Default)]
pub struct SharedCache {
    sources: RwLock<HashMap<ModuleId, (Stamp, Arc<ModuleSource>)>>,
    code: RwLock<CodeEntries>,
    code_bytes: AtomicUsize,
    accepted: AtomicU32,
    rejected: AtomicU32,
}

impl SharedCache {
    /// The process-wide instance, once [`SharedCache::enable_global`] ran; `None` means off.
    pub fn global() -> Option<Arc<Self>> {
        GLOBAL.get().cloned()
    }

    /// Turns the process-wide layer on (the CLI flag). Idempotent.
    pub fn enable_global() -> Arc<Self> {
        Arc::clone(GLOBAL.get_or_init(Arc::default))
    }

    /// The stored source, unless the file changed since it was read.
    pub(crate) fn source(&self, id: &ModuleId) -> Option<Arc<ModuleSource>> {
        let (seen, source) = self.sources.read().unwrap().get(id).cloned()?;
        (seen.is_none() || seen == stamp(id)).then_some(source)
    }

    /// Stores `source`, replacing an entry whose stamp is stale; returns what every isolate now
    /// sees.
    pub(crate) fn insert_source(&self, id: &ModuleId, source: ModuleSource) -> Arc<ModuleSource> {
        let now = stamp(id);
        let mut sources = self.sources.write().unwrap();
        if let Some((seen, kept)) = sources.get(id)
            && *seen == now
        {
            return Arc::clone(kept);
        }
        let source = Arc::new(source);
        sources.insert(id.clone(), (now, Arc::clone(&source)));
        source
    }

    /// Sealed code cache of module `id` whose text has `hash` (`ModuleSource::hash`).
    pub fn code(&self, id: &ModuleId, hash: &str) -> Option<Arc<[u8]>> {
        let code = self.code.read().unwrap();
        code.get(&(id.clone(), hash.to_owned())).cloned()
    }

    /// Stores `blob` unless an entry exists; the first producer wins.
    pub fn insert_code(&self, id: &ModuleId, hash: &str, blob: Arc<[u8]>) {
        let mut code = self.code.write().unwrap();
        code.entry((id.clone(), hash.to_owned()))
            .or_insert_with(|| {
                self.code_bytes.fetch_add(blob.len(), Ordering::Relaxed);
                blob
            });
    }

    /// Adds what one isolate consumed: caches the engine accepted and rejected.
    pub fn record_uses(&self, accepted: u32, rejected: u32) {
        self.accepted.fetch_add(accepted, Ordering::Relaxed);
        self.rejected.fetch_add(rejected, Ordering::Relaxed);
    }

    /// `(accepted, rejected)` code caches over every isolate so far.
    pub fn uses(&self) -> (u32, u32) {
        (
            self.accepted.load(Ordering::Relaxed),
            self.rejected.load(Ordering::Relaxed),
        )
    }

    /// Total size of the stored code caches.
    pub fn code_bytes(&self) -> usize {
        self.code_bytes.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use velyr_runtime_errors::ErrorCode;
    use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

    use super::SharedCache;
    use crate::{
        BoxFuture, LoadContext, ModuleHost, ModuleId, ModuleLoader, ModuleSource, ResolvedModule,
        Result,
    };

    struct Counting(Arc<AtomicUsize>);

    impl ModuleLoader for Counting {
        fn load<'a>(
            &'a self,
            _module: &'a ResolvedModule,
            _ctx: &'a LoadContext,
        ) -> BoxFuture<'a, Result<ModuleSource>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(ModuleSource::new("export const a = 1;".into())) })
        }
    }

    fn module() -> ResolvedModule {
        let path = std::path::PathBuf::from("/app/main.js");
        ResolvedModule {
            id: ModuleId::from_path(&path).unwrap(),
            path,
        }
    }

    fn ctx(allow: bool) -> LoadContext {
        let mut builder = CapabilitySet::builder();
        if allow {
            builder = builder.allow(Capability::ImportLocal(PathPattern::new("/app")));
        }
        LoadContext {
            permissions: builder.build(),
        }
    }

    fn host(shared: &Arc<SharedCache>, reads: &Arc<AtomicUsize>) -> ModuleHost {
        ModuleHost::new(Arc::new(Counting(reads.clone()))).with_shared(shared.clone())
    }

    #[tokio::test]
    async fn second_host_reads_the_source_from_the_shared_layer() {
        let (shared, reads) = (Arc::new(SharedCache::default()), Arc::default());
        let first = host(&shared, &reads)
            .load(&module(), &ctx(true))
            .await
            .unwrap();
        let second = host(&shared, &reads)
            .load(&module(), &ctx(true))
            .await
            .unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn shared_hit_still_requires_permission() {
        let (shared, reads) = (Arc::new(SharedCache::default()), Arc::default());
        host(&shared, &reads)
            .load(&module(), &ctx(true))
            .await
            .unwrap();
        let err = host(&shared, &reads)
            .load(&module(), &ctx(false))
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::PermissionDenied);
    }

    #[tokio::test]
    async fn file_edited_between_workers_is_seen_by_the_second() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("main.js");
        std::fs::write(&path, "export const a = 1;").unwrap();
        let module = ResolvedModule {
            id: ModuleId::from_path(&path).unwrap(),
            path,
        };
        let shared = Arc::new(SharedCache::default());
        let worker = || ModuleHost::new(Arc::new(crate::FsLoader)).with_shared(shared.clone());
        let first = worker().load(&module, &ctx_for(dir.path())).await.unwrap();
        std::fs::write(&module.path, "export const a = 22;").unwrap();
        let second = worker().load(&module, &ctx_for(dir.path())).await.unwrap();
        assert_eq!(first.text, "export const a = 1;");
        assert_eq!(second.text, "export const a = 22;");
    }

    fn ctx_for(dir: &std::path::Path) -> LoadContext {
        let dir = dir.canonicalize().unwrap();
        LoadContext {
            permissions: CapabilitySet::builder()
                .allow(Capability::ImportLocal(PathPattern::new(dir.to_string_lossy())))
                .build(),
        }
    }

    #[test]
    fn code_entries_are_set_once_and_keyed_by_content_hash() {
        let cache = SharedCache::default();
        let id = module().id;
        assert!(cache.code(&id, "h1").is_none());
        cache.insert_code(&id, "h1", Arc::from(&b"first"[..]));
        cache.insert_code(&id, "h1", Arc::from(&b"second"[..]));
        assert_eq!(&*cache.code(&id, "h1").unwrap(), b"first");
        assert!(cache.code(&id, "h2").is_none());
        assert_eq!(cache.code_bytes(), 5);
    }

    #[test]
    fn global_is_off_until_enabled_and_then_one_instance() {
        let first = SharedCache::enable_global();
        assert!(Arc::ptr_eq(&first, &SharedCache::enable_global()));
        assert!(Arc::ptr_eq(&first, &SharedCache::global().unwrap()));
    }
}

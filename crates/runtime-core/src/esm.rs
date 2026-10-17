//! ES module entry: the graph is loaded through `velyr-runtime-modules` (permission-gated,
//! asynchronous), linked and evaluated by the engine, including top-level await and `import()`.
//!
//! The engine links synchronously through a [`ModuleResolveHook`], so the hook only serves
//! sources that were already read. Before linking, the loader reads the whole graph its import
//! scan can see, many modules at a time, so a graph is usually linked once. When the hook meets a
//! module the scan missed, it records it and fails; the loader reads it and links again. Edges the
//! engine already resolved are cached, so every retry makes progress.
//!
//! `import()` reaches the host through the `op_module_import` function, which queues a request;
//! the loop in [`drive`] serves requests between event-loop runs.

mod prefetch;
mod scan;
mod sync;

use std::path::Path;
use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::Instrument;
use velyr_engine_api::{
    CallContext, Handle, HostError, HostFunctionDef, Isolate, JsException, JsString, MappedPosition,
    ModuleResolveHook, PromiseState, ResolverId, Source, Value,
};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::{CompletionSender, EventLoop, OpGuard};
use velyr_runtime_modules::{
    FsLoader, FsResolver, LoadContext, ModuleHost, ModuleId, ModuleLoader, ModuleSource,
    ResolveContext, ResolvedModule, SharedCache, SourceTransform,
};
use velyr_runtime_permissions::CapabilitySet;

mod code_cache;

use sync::SourceCache;
pub use sync::{EsmRequire, require_esm};

use crate::event_loop::LoopIsolate;
use crate::watchdog::Watchdog;
use crate::{ModuleResolverHandle, RunError, run_error};

/// Name the engine adapter looks up to deliver `import()` requests.
const IMPORT_OP: &str = "op_module_import";

/// JS helpers kept as one function so no global is added: `(op, a, b)`.
/// 0: new Error(a) with `code` b; 1: message of a thrown value; 2: its stack.
const HELPERS_JS: &str = r#"(function (op, a, b) {
  "use strict";
  switch (op) {
    case 0: {
      const error = new Error(a);
      if (b) error.code = b;
      return error;
    }
    case 1:
      try {
        return a instanceof Error ? (a.code ? a.code + ": " : "") + a.message : String(a);
      } catch {
        return "[unprintable value]";
      }
    default:
      return a instanceof Error && typeof a.stack === "string" ? a.stack : undefined;
  }
})"#;

struct ImportRequest {
    specifier: String,
    referrer: String,
    resolver: ResolverId,
    /// Keeps the event loop alive until the import settles.
    guard: OpGuard,
}

/// Isolate slot the `op_module_import` host function reaches.
struct ImportQueue {
    jobs: UnboundedSender<ImportRequest>,
    sender: CompletionSender,
}

fn op_module_import(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (Some(Value::String(specifier)), Some(Value::String(referrer)), Some(Value::Number(id))) =
        (args.first(), args.get(1), args.get(2))
    else {
        return Err(ctx.throw_type_error("op_module_import expects (specifier, referrer, id)"));
    };
    let request = |guard| ImportRequest {
        specifier: specifier.to_string_lossy(),
        referrer: referrer.to_string_lossy(),
        resolver: ResolverId(*id as u64),
        guard,
    };
    let queue = ctx
        .isolate_slot::<ImportQueue>()
        .ok_or_else(|| HostError::TypeError("import() needs an installed module host".into()))?;
    let request = request(queue.sender.begin_op());
    queue
        .jobs
        .send(request)
        .map_err(|_| HostError::TypeError("module host is shut down".into()))?;
    Ok(Value::Undefined)
}

enum LoadError {
    Velyr(VelyrError),
    Js(JsException),
}

impl From<VelyrError> for LoadError {
    fn from(err: VelyrError) -> Self {
        Self::Velyr(err)
    }
}

/// Resolution, permission-gated loading and the sources the engine hook may serve.
struct Modules {
    host: Arc<ModuleHost>,
    load: LoadContext,
    resolve_ctx: ResolveContext,
    resolver: ModuleResolverHandle,
    loaded: SourceCache,
    /// Process-wide immutable layer, when `--experimental-shared-code-cache` is on.
    shared: Option<Arc<SharedCache>>,
    /// Referrer for requests that do not come from a module (e.g. classic scripts).
    entry: ModuleId,
}

impl Modules {
    fn resolve(&self, specifier: &str, referrer: &ModuleId) -> Result<ResolvedModule, VelyrError> {
        self.resolver
            .resolve(specifier, referrer, &self.resolve_ctx)
    }

    async fn fetch(&mut self, module: &ResolvedModule) -> Result<Arc<ModuleSource>, VelyrError> {
        let source = self.host.load(module, &self.load).await?;
        self.loaded
            .lock()
            .unwrap()
            .insert(module.id.clone(), Arc::clone(&source));
        Ok(source)
    }

    /// Reads, compiles and links `root` with everything it imports. The caller releases the handle.
    async fn load_graph(
        &mut self,
        isolate: &mut impl Isolate,
        root: &ResolvedModule,
    ) -> Result<Handle, LoadError> {
        let span = tracing::debug_span!("velyr.core.load_graph");
        async {
            let source = self.fetch(root).await?;
            self.prefetch(root, &source).await;
            let cache = self
                .shared
                .as_ref()
                .and_then(|shared| shared.code(&root.id, &source.hash));
            let handle = isolate
                .compile_module_cached(
                    &Source {
                        name: root.id.as_str().to_owned(),
                        text: source.text.clone(),
                    },
                    cache.as_deref(),
                )
                .map_err(LoadError::Js)?;
            if let Err(err) = self.link(isolate, handle).await {
                isolate.release(handle);
                return Err(err);
            }
            Ok(handle)
        }
        .instrument(span)
        .await
    }

    async fn link(&mut self, isolate: &mut impl Isolate, handle: Handle) -> Result<(), LoadError> {
        loop {
            let mut hook = LinkHook::new(self);
            tracing::debug!(target: "velyr.core.instantiate", "instantiate");
            let linked = isolate.instantiate_module(handle, &mut hook);
            let (missing, failure) = (hook.missing, hook.failure);
            match (linked, missing, failure) {
                (Ok(()), ..) => return Ok(()),
                (Err(_), Some(module), _) => {
                    self.fetch(&module).await?;
                }
                (Err(_), None, Some(err)) => return Err(err.into()),
                (Err(exception), None, None) => return Err(LoadError::Js(exception)),
            }
        }
    }

    /// Loads the graph behind `import(specifier)`; returns the module name the engine knows it by.
    async fn import(
        &mut self,
        isolate: &mut impl Isolate,
        specifier: &str,
        referrer: &str,
    ) -> Result<String, LoadError> {
        let referrer = ModuleId::parse(referrer).unwrap_or_else(|| self.entry.clone());
        let module = self.resolve(specifier, &referrer)?;
        let handle = self.load_graph(isolate, &module).await?;
        isolate.release(handle);
        Ok(module.id.as_str().to_owned())
    }
}

/// Serves already-read sources to the engine; remembers what stopped it.
struct LinkHook<'a> {
    modules: &'a Modules,
    missing: Option<ResolvedModule>,
    failure: Option<VelyrError>,
}

impl<'a> LinkHook<'a> {
    fn new(modules: &'a Modules) -> Self {
        Self {
            modules,
            missing: None,
            failure: None,
        }
    }

    fn fail(&mut self, err: VelyrError) -> HostError {
        self.failure = Some(err);
        HostError::Velyr(VelyrError::internal("module resolution failed"))
    }
}

impl ModuleResolveHook for LinkHook<'_> {
    fn code_cache(&mut self, name: &str) -> Option<Arc<[u8]>> {
        let shared = self.modules.shared.as_ref()?;
        let id = ModuleId::parse(name)?;
        let hash = self.modules.loaded.lock().unwrap().get(&id)?.hash.clone();
        shared.code(&id, &hash)
    }

    fn resolve(&mut self, specifier: &str, referrer: &str) -> Result<Source, HostError> {
        let Some(referrer) = ModuleId::parse(referrer) else {
            return Err(self.fail(VelyrError::invalid_data(format!(
                "referrer is not a module URL: {referrer}"
            ))));
        };
        let module = self
            .modules
            .resolve(specifier, &referrer)
            .map_err(|err| self.fail(err))?;
        let cached = self.modules.loaded.lock().unwrap().get(&module.id).cloned();
        match cached {
            Some(source) => Ok(Source {
                name: module.id.as_str().to_owned(),
                text: source.text.clone(),
            }),
            None => {
                self.missing = Some(module);
                Err(HostError::Velyr(VelyrError::internal(
                    "module not read yet",
                )))
            }
        }
    }
}

/// Builds and reads back JS values the engine contract cannot (errors with a `code`).
struct Helpers(Value);

impl Helpers {
    fn install(isolate: &mut impl Isolate) -> Result<Self, RunError> {
        let function = isolate
            .eval_script(&Source {
                name: "<module-helpers>".into(),
                text: HELPERS_JS.into(),
            })
            .map_err(|err| RunError::Setup(format!("module helpers: {}", err.message)))?;
        Ok(Self(function))
    }

    fn call(&self, isolate: &mut impl Isolate, args: &[Value]) -> Value {
        isolate
            .call(&self.0, &Value::Undefined, args)
            .unwrap_or(Value::Undefined)
    }

    fn error(&self, isolate: &mut impl Isolate, message: &str, code: Option<&str>) -> Value {
        let text = |s: &str| Value::String(JsString::from(s));
        let code = code.map_or(Value::Undefined, text);
        self.call(isolate, &[Value::Number(0.0), text(message), code])
    }

    fn rejection(&self, isolate: &mut impl Isolate, err: LoadError) -> Value {
        match err {
            LoadError::Velyr(err) => self.error(isolate, err.message(), Some(err.code().as_str())),
            LoadError::Js(exception) => match exception.value {
                Some(value) => value,
                None => self.error(isolate, &exception.message, None),
            },
        }
    }

    fn describe(&self, isolate: &mut impl Isolate, reason: Value) -> JsException {
        let text = |value: Value| match value {
            Value::String(text) => Some(text.to_string_lossy()),
            _ => None,
        };
        let message = text(self.call(isolate, &[Value::Number(1.0), reason.clone()]));
        let stack = text(self.call(isolate, &[Value::Number(2.0), reason.clone()]));
        JsException {
            message: message.unwrap_or_else(|| "unknown error".into()),
            stack,
            value: Some(reason),
        }
    }
}

fn load_error(err: LoadError, watchdog: Option<&Watchdog>) -> RunError {
    match err {
        LoadError::Velyr(err) => RunError::Load(err),
        LoadError::Js(exception) => run_error(exception, watchdog),
    }
}

/// Loads, links and evaluates the module at `path`, then drives the loop until idle.
pub(crate) async fn run<I: Isolate>(
    event_loop: &mut EventLoop<LoopIsolate<I>>,
    watchdog: Option<&Watchdog>,
    capabilities: Option<Arc<CapabilitySet>>,
    resolver: Option<ModuleResolverHandle>,
    source_transform: Option<Arc<dyn SourceTransform>>,
    loader: Option<Arc<dyn ModuleLoader>>,
    path: &Path,
) -> Result<(), RunError> {
    let (jobs, requests) = unbounded_channel();
    let sender = event_loop.sender();
    let isolate = event_loop.isolate();
    install_import_op(isolate, ImportQueue { jobs, sender })?;
    let helpers = Helpers::install(isolate)?;

    let absolute = std::path::absolute(path)
        .map_err(|err| RunError::Load(VelyrError::invalid_data(err.to_string())))?;
    let entry = ModuleId::from_path(&absolute)
        .ok_or_else(|| RunError::Load(VelyrError::invalid_data("entry path is not absolute")))?;
    let permissions = capabilities.unwrap_or_default();
    let mut require = EsmRequire::new(resolver.clone(), Arc::clone(&permissions));
    let mut host = ModuleHost::new(loader.unwrap_or_else(|| Arc::new(FsLoader)));
    let shared = SharedCache::global();
    if let Some(shared) = &shared {
        host = host.with_shared(Arc::clone(shared));
    }
    if let Some(transform) = source_transform {
        require = require.with_transform(Arc::clone(&transform));
        host = host.with_transform(transform);
    }
    let mut modules = Modules {
        host: Arc::new(host),
        load: LoadContext { permissions },
        resolve_ctx: ResolveContext::default(),
        resolver: resolver.unwrap_or_else(|| Arc::new(FsResolver)),
        loaded: SourceCache::default(),
        shared,
        entry,
    };
    let to_original = modules.host.source_maps().mapper();
    isolate.set_entry_module(modules.entry.as_str());
    isolate.set_prepare_stack_trace(Arc::new(move |file, line, column| {
        to_original(file, line, column).map(|(file, line, column)| MappedPosition {
            file,
            line,
            column,
        })
    }));
    // `require(esm)` reads into the same cache, so both sides see one instance per module.
    require
        .sharing(Arc::clone(&modules.loaded))
        .install(isolate);

    let entry_promise = start(isolate, &mut modules, watchdog, &absolute).await?;
    entry_outcome(isolate, &helpers, &entry_promise, false)?;
    drive(event_loop, &mut modules, &helpers, watchdog, requests).await;
    entry_outcome(event_loop.isolate(), &helpers, &entry_promise, true)?;
    check_rejections(event_loop.isolate(), &helpers)
}

fn install_import_op(
    isolate: &mut LoopIsolate<impl Isolate>,
    queue: ImportQueue,
) -> Result<(), RunError> {
    isolate.set_slot(queue);
    isolate
        .register_host_function(HostFunctionDef {
            name: IMPORT_OP,
            arity: 3,
            callback: op_module_import,
        })
        .map_err(|err| RunError::Setup(err.to_string()))?;
    // The engine finds the op by name; user code must not be able to call it.
    isolate
        .eval_script(&Source {
            name: "<module-host>".into(),
            text: format!("delete globalThis.{IMPORT_OP}"),
        })
        .map_err(|err| RunError::Setup(err.message))?;
    Ok(())
}

async fn start(
    isolate: &mut impl Isolate,
    modules: &mut Modules,
    watchdog: Option<&Watchdog>,
    path: &Path,
) -> Result<Value, RunError> {
    let specifier = path.to_string_lossy();
    let root = modules
        .resolve(&specifier, &modules.entry)
        .map_err(RunError::Load)?;
    let handle = modules
        .load_graph(isolate, &root)
        .await
        .map_err(|err| load_error(err, watchdog))?;
    let _tick = watchdog.map(Watchdog::begin_tick);
    let promise = isolate
        .evaluate_module(handle)
        .map_err(|exception| run_error(exception, watchdog));
    isolate.release(handle);
    isolate.perform_microtask_checkpoint();
    if let Some(shared) = &modules.shared {
        code_cache::publish(isolate, shared, &modules.loaded);
    }
    promise
}

/// A rejected entry fails the run; with `settled`, a pending one does too (nothing can wake it).
fn entry_outcome(
    isolate: &mut impl Isolate,
    helpers: &Helpers,
    promise: &Value,
    settled: bool,
) -> Result<(), RunError> {
    match isolate.promise_state(promise) {
        Some(PromiseState::Rejected(reason)) => {
            Err(RunError::Uncaught(helpers.describe(isolate, reason)))
        }
        Some(PromiseState::Pending) if settled => Err(RunError::UnsettledEntry),
        _ => Ok(()),
    }
}

fn check_rejections(isolate: &mut impl Isolate, helpers: &Helpers) -> Result<(), RunError> {
    let reasons = isolate.take_unhandled_rejections();
    match reasons.into_iter().next() {
        Some(reason) => Err(RunError::UnhandledRejection(
            helpers.describe(isolate, reason).message,
        )),
        None => Ok(()),
    }
}

/// Runs the event loop, serving `import()` requests whenever the loop is not running.
async fn drive<I: Isolate>(
    event_loop: &mut EventLoop<LoopIsolate<I>>,
    modules: &mut Modules,
    helpers: &Helpers,
    watchdog: Option<&Watchdog>,
    mut requests: UnboundedReceiver<ImportRequest>,
) {
    loop {
        tokio::select! {
            biased;
            Some(request) = requests.recv() => {
                serve_import(event_loop.isolate(), modules, helpers, watchdog, request).await;
            }
            () = event_loop.run() => {
                if requests.is_empty() || event_loop.sender().is_shutdown() {
                    return;
                }
            }
        }
    }
}

// shortcut: requests are served one at a time and the loop (timers) pauses while a graph is
// read from disk; upgrade by polling loads next to `EventLoop::run` once reads get slow.
async fn serve_import(
    isolate: &mut impl Isolate,
    modules: &mut Modules,
    helpers: &Helpers,
    watchdog: Option<&Watchdog>,
    request: ImportRequest,
) {
    let ImportRequest {
        specifier,
        referrer,
        resolver,
        guard,
    } = request;
    let outcome = modules.import(isolate, &specifier, &referrer).await;
    let _tick = watchdog.map(Watchdog::begin_tick);
    match outcome {
        Ok(name) => isolate.resolve(resolver, Value::String(JsString::from(name.as_str()))),
        Err(err) => {
            let reason = helpers.rejection(isolate, err);
            isolate.reject(resolver, reason);
        }
    }
    drop(guard);
}

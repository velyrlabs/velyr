//! Runs a script on a dedicated isolate thread driven by the Velyr event loop (ADR-010).

use std::any::Any;
use std::path::PathBuf;
use std::thread::{self, ThreadId};

use velyr_engine_api::{
    CodeCacheStats, Engine, EngineError, GcReason, Handle, HeapStats, HostFunctionDef, Isolate,
    JsException, MicrotaskProgress, ModuleResolveHook, PromiseState, ResolverId, Source,
    TerminationHandle, Value,
};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::{CompletionSender, EventLoop, TimerHandle};

use crate::{RunError, Runtime, RuntimeConfig, esm, eval_file};

/// An isolate owned by the event loop. Every engine call asserts (debug builds) that it runs
/// on the thread that created it: no engine mutation outside the isolate thread.
pub struct LoopIsolate<I> {
    inner: I,
    owner: ThreadId,
    /// Nesting of JS turns; only the outermost one is timed.
    depth: u32,
    /// Compat-layer queue (e.g. `process.nextTick`) drained before each promise checkpoint.
    tick_hook: Option<Value>,
}

impl<I> LoopIsolate<I> {
    fn new(inner: I) -> Self {
        Self {
            inner,
            owner: thread::current().id(),
            depth: 0,
            tick_hook: None,
        }
    }

    /// Sets a JS function called before every microtask checkpoint; it returns `true` when it
    /// ran something. The checkpoint repeats until both the hook and the microtasks are idle.
    pub fn set_tick_hook(&mut self, hook: Value) {
        self.tick_hook = Some(hook);
    }

    /// Runs JS-executing engine work as one turn for the loop-lag and heap metrics.
    fn turn<R>(&mut self, work: impl FnOnce(&mut I) -> R) -> R
    where
        I: Isolate,
    {
        self.on_owner();
        let metrics = crate::metrics::global();
        if self.depth == 0 {
            metrics.turn_start();
        }
        self.depth += 1;
        let result = work(&mut self.inner);
        self.depth -= 1;
        if self.depth == 0 {
            metrics.turn_end();
            metrics.record_heap(self.inner.heap_stats());
        }
        result
    }

    fn on_owner(&self) {
        debug_assert_eq!(
            self.owner,
            thread::current().id(),
            "engine touched off the isolate thread"
        );
    }
}

impl<I: Isolate> Isolate for LoopIsolate<I> {
    fn eval_script(&mut self, source: &Source) -> Result<Value, JsException> {
        self.turn(|inner| inner.eval_script(source))
    }

    fn compile_module(&mut self, source: &Source) -> Result<Handle, JsException> {
        self.on_owner();
        self.inner.compile_module(source)
    }

    fn compile_module_cached(
        &mut self,
        source: &Source,
        cache: Option<&[u8]>,
    ) -> Result<Handle, JsException> {
        self.on_owner();
        self.inner.compile_module_cached(source, cache)
    }

    fn create_code_cache(&mut self, name: &str) -> Option<Vec<u8>> {
        self.on_owner();
        self.inner.create_code_cache(name)
    }

    fn code_cache_stats(&self) -> CodeCacheStats {
        self.inner.code_cache_stats()
    }

    fn instantiate_module(
        &mut self,
        module: Handle,
        hook: &mut dyn ModuleResolveHook,
    ) -> Result<(), JsException> {
        self.on_owner();
        self.inner.instantiate_module(module, hook)
    }

    fn evaluate_module(&mut self, module: Handle) -> Result<Value, JsException> {
        self.turn(|inner| inner.evaluate_module(module))
    }

    fn call(&mut self, func: &Value, this: &Value, args: &[Value]) -> Result<Value, JsException> {
        self.turn(|inner| inner.call(func, this, args))
    }

    fn register_host_function(&mut self, def: HostFunctionDef) -> Result<(), EngineError> {
        self.on_owner();
        self.inner.register_host_function(def)
    }

    fn set_slot<T: Any>(&mut self, value: T) {
        self.on_owner();
        self.inner.set_slot(value);
    }

    fn set_prepare_stack_trace(&mut self, mapper: velyr_engine_api::StackFrameMapper) {
        self.on_owner();
        self.inner.set_prepare_stack_trace(mapper);
    }

    fn set_entry_module(&mut self, name: &str) {
        self.on_owner();
        self.inner.set_entry_module(name);
    }

    fn create_promise(&mut self) -> (Value, ResolverId) {
        self.on_owner();
        self.inner.create_promise()
    }

    fn resolve(&mut self, resolver: ResolverId, value: Value) {
        self.on_owner();
        self.inner.resolve(resolver, value);
    }

    fn reject(&mut self, resolver: ResolverId, reason: Value) {
        self.on_owner();
        self.inner.reject(resolver, reason);
    }

    fn create_array_buffer(&mut self, bytes: &[u8]) -> Value {
        self.on_owner();
        self.inner.create_array_buffer(bytes)
    }

    fn create_velyr_error(&mut self, err: &VelyrError) -> Value {
        self.on_owner();
        self.inner.create_velyr_error(err)
    }

    fn promise_state(&mut self, promise: &Value) -> Option<PromiseState> {
        self.on_owner();
        self.inner.promise_state(promise)
    }

    fn perform_microtask_checkpoint(&mut self) -> MicrotaskProgress {
        let Some(hook) = self.tick_hook.clone() else {
            return self.turn(|inner| inner.perform_microtask_checkpoint());
        };
        self.turn(|inner| {
            // `ran` can't come from the engine's progress flag: calling the hook marks it dirty.
            let call_hook = |inner: &mut I| match inner.call(&hook, &Value::Undefined, &[]) {
                Ok(value) => value == Value::Bool(true),
                Err(err) => {
                    tracing::warn!(message = %err.message, "tick hook threw");
                    false
                }
            };
            // Ticks first, then promises; ticks queued by promises run before the next ones.
            let mut ticked = call_hook(inner);
            let mut microtasks = inner.perform_microtask_checkpoint();
            while call_hook(inner) {
                ticked = true;
                microtasks = inner.perform_microtask_checkpoint();
            }
            if ticked {
                MicrotaskProgress::Progressed
            } else {
                microtasks
            }
        })
    }

    fn take_unhandled_rejections(&mut self) -> Vec<Value> {
        self.on_owner();
        self.inner.take_unhandled_rejections()
    }

    fn heap_stats(&self) -> HeapStats {
        self.inner.heap_stats()
    }

    fn request_gc(&mut self, reason: GcReason) {
        self.on_owner();
        self.inner.request_gc(reason);
        // An idle GC runs outside any turn; without this the gauge keeps the pre-GC heap.
        crate::metrics::global().record_heap(self.inner.heap_stats());
    }

    fn run_platform_tasks(&mut self) -> bool {
        self.on_owner();
        self.inner.run_platform_tasks()
    }

    fn termination_handle(&self) -> TerminationHandle {
        self.inner.termination_handle()
    }

    fn release(&mut self, handle: Handle) {
        self.on_owner();
        self.inner.release(handle);
    }

    fn scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.on_owner();
        f(self)
    }
}

/// How the entry file is executed.
#[derive(Clone, Copy)]
enum Entry {
    /// A classic script (internal API only).
    Script,
    /// An ES module: static imports, top-level await and `import()`.
    Module,
}

/// Bootstraps an isolate on a dedicated thread (Tokio `current_thread` runtime), lets `install`
/// put runtime state (e.g. op slots, the loop's timer handle) on it, runs the script at `path`, then drives the event
/// loop until idle. Unhandled promise rejections fail the run.
pub fn run_file<E, F>(
    engine: E,
    config: RuntimeConfig,
    host_functions: Vec<HostFunctionDef>,
    bootstrap_js: &'static str,
    install: F,
    path: PathBuf,
) -> Result<(), RunError>
where
    E: Engine + Send + 'static,
    F: FnOnce(&mut LoopIsolate<E::Isolate>, CompletionSender, TimerHandle) + Send + 'static,
{
    run_entry(
        Entry::Script,
        engine,
        config,
        host_functions,
        bootstrap_js,
        install,
        path,
    )
}

/// Like [`run_file`], but `path` is an ES module: its imports are loaded through
/// `velyr-runtime-modules` under `config.capabilities`, top-level await is supported and
/// `import()` works. Load, permission and resolution failures carry `VELYR_*` codes.
pub fn run_module_file<E, F>(
    engine: E,
    config: RuntimeConfig,
    host_functions: Vec<HostFunctionDef>,
    bootstrap_js: &'static str,
    install: F,
    path: PathBuf,
) -> Result<(), RunError>
where
    E: Engine + Send + 'static,
    F: FnOnce(&mut LoopIsolate<E::Isolate>, CompletionSender, TimerHandle) + Send + 'static,
{
    run_entry(
        Entry::Module,
        engine,
        config,
        host_functions,
        bootstrap_js,
        install,
        path,
    )
}

fn run_entry<E, F>(
    entry: Entry,
    engine: E,
    config: RuntimeConfig,
    host_functions: Vec<HostFunctionDef>,
    bootstrap_js: &'static str,
    install: F,
    path: PathBuf,
) -> Result<(), RunError>
where
    E: Engine + Send + 'static,
    F: FnOnce(&mut LoopIsolate<E::Isolate>, CompletionSender, TimerHandle) + Send + 'static,
{
    // The isolate runs on its own thread; a server worker's metrics identity follows it.
    let slot = crate::metrics::Metrics::worker_slot();
    let worker = thread::Builder::new()
        .name("velyr-isolate".into())
        .spawn(move || {
            slot.enter();
            run_on_isolate_thread(
                entry,
                engine,
                config,
                host_functions,
                bootstrap_js,
                install,
                path,
            )
        })
        .map_err(|err| RunError::Setup(err.to_string()))?;
    worker
        .join()
        .unwrap_or_else(|_| Err(RunError::Setup("isolate thread panicked".into())))
}

fn run_on_isolate_thread<E, F>(
    entry: Entry,
    engine: E,
    config: RuntimeConfig,
    host_functions: Vec<HostFunctionDef>,
    bootstrap_js: &str,
    install: F,
    path: PathBuf,
) -> Result<(), RunError>
where
    E: Engine,
    F: FnOnce(&mut LoopIsolate<E::Isolate>, CompletionSender, TimerHandle),
{
    let (leak_check, collector) = (config.leak_check, config.leak_collector);
    // The isolate (and its slots) is dropped when `run_isolate` returns.
    let result = run_isolate(
        entry,
        engine,
        config,
        host_functions,
        bootstrap_js,
        install,
        path,
    );
    let leaks = collector.map(|collect| collect()).unwrap_or_default();
    if !leaks.is_empty() {
        tracing::debug!(target: "velyr.runtime_core.leak", open = leaks.len(), "velyr.runtime_core.leak");
    }
    match result {
        Ok(()) | Err(RunError::Terminated) if leak_check && !leaks.is_empty() => {
            Err(RunError::Leak(leaks))
        }
        other => other,
    }
}

fn run_isolate<E, F>(
    entry: Entry,
    engine: E,
    config: RuntimeConfig,
    host_functions: Vec<HostFunctionDef>,
    bootstrap_js: &str,
    install: F,
    path: PathBuf,
) -> Result<(), RunError>
where
    E: Engine,
    F: FnOnce(&mut LoopIsolate<E::Isolate>, CompletionSender, TimerHandle),
{
    let max_completions = config.max_completions_per_tick;
    let capabilities = config.capabilities.clone();
    let resolver = config.module_resolver.clone();
    let source_transform = config.source_transform.clone();
    let module_loader = config.module_loader.clone();
    let shutdown = config.shutdown.clone().unwrap_or_default();
    let runtime = Runtime::with_engine(engine, config);
    let mut isolate = runtime
        .create_isolate()
        .map_err(|err| RunError::Setup(err.to_string()))?;
    isolate
        .bootstrap(&host_functions, bootstrap_js)
        .map_err(|err| RunError::Setup(err.to_string()))?;
    let (isolate, watchdog) = isolate.into_parts();
    let mut event_loop = EventLoop::new(LoopIsolate::new(isolate), shutdown.clone())
        .with_max_completions_per_tick(max_completions);
    let sender = event_loop.sender();
    let timers = event_loop.timer_handle();
    install(event_loop.isolate(), sender, timers);

    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|err| RunError::Setup(err.to_string()))?;
    let result = tokio.block_on(async {
        if let Entry::Module = entry {
            return esm::run(
                &mut event_loop,
                watchdog.as_ref(),
                capabilities,
                resolver,
                source_transform,
                module_loader,
                &path,
            )
            .await;
        }
        eval_file(event_loop.isolate(), watchdog.as_ref(), &path)?;
        check_rejections(event_loop.isolate())?;
        event_loop.run().await;
        // shortcut: rejections are checked after the script and when the loop goes idle,
        // not after every tick; upgrade by letting `EventLoop::run` call a per-tick hook.
        check_rejections(event_loop.isolate())
    });
    if !shutdown.is_cancelled() {
        return result;
    }
    // Whatever the engine reported after cancellation is an effect of the termination.
    event_loop.terminate();
    Err(RunError::Terminated)
}

fn check_rejections(isolate: &mut impl Isolate) -> Result<(), RunError> {
    match isolate.take_unhandled_rejections().first() {
        Some(reason) => Err(RunError::UnhandledRejection(render(reason))),
        None => Ok(()),
    }
}

fn render(reason: &Value) -> String {
    match reason {
        Value::String(text) => text.to_string_lossy(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Undefined => "undefined".into(),
        Value::Null => "null".into(),
        Value::BigInt(_) | Value::Object(_) => "[non-string reason]".into(),
    }
}

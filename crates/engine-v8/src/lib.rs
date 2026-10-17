//! V8 adapter for `velyr-engine-api` (ADR-009). No `v8::` type appears in the public API.

mod code_cache;
mod continuation;
mod convert;
mod handles;
mod heap;
mod host;
mod inspector;
mod modules;
mod platform;
mod serialize;
mod shared;
mod snapshot;
mod stack_trace;

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Once};

use velyr_engine_api::{
    CodeCacheStats, Engine, EngineError, EngineFeatures, GcReason, Handle, HeapStats,
    HostFunctionDef, Isolate, IsolateOptions, JsException, MicrotaskProgress, ModuleResolveHook,
    PlatformWaker, PromiseState, ResolverId, SerializedValue, Source, TerminationHandle, Value,
};
use velyr_runtime_errors::VelyrError;

use convert::{exception_from, from_v8, internal_exception, to_v8};
use heap::HeapGuard;
use host::SharedSlot;
use shared::Shared;

pub use inspector::{InspectorEvent, InspectorOptions};
pub use snapshot::{SNAPSHOT_RESULTS_KEY, create_snapshot};

static PLATFORM_INIT: Once = Once::new();

/// Caps the young generation at 16 MB per semi-space (V8 defaults to 64 MB): -10% peak
/// footprint with one worker and -17% with four on the large-app bench, same CPU per request
/// (VELYR-212, docs/research/large-app-footprint.md). Smaller caps cost throughput.
const V8_FLAGS: &str = "--max-semi-space-size=16";

/// Factory for [`V8Isolate`]s.
#[derive(Debug, Default, Clone)]
pub struct V8Engine {
    snapshot: Option<&'static [u8]>,
    /// The snapshot's host functions, bound again on every isolate it boots.
    snapshot_ops: Arc<[HostFunctionDef]>,
    inspector: Option<inspector::PendingInspector>,
    shared_memory: bool,
    disallow_codegen: bool,
}

impl V8Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Boots every isolate's context from `blob`, made by [`create_snapshot`] with this same V8
    /// build and flags (V8 aborts the process on a mismatched blob) and these `host_functions`,
    /// in the same order: `create_isolate` fails when the names differ.
    pub fn with_snapshot(blob: &'static [u8], host_functions: Vec<HostFunctionDef>) -> Self {
        Self {
            snapshot: Some(blob),
            snapshot_ops: host_functions.into(),
            ..Self::default()
        }
    }

    /// Opt-in (default off): `serialize`/`deserialize` may move `SharedArrayBuffer`s between
    /// isolates, which then share one writable memory. Without it they are a `DataCloneError`.
    pub fn with_shared_memory(mut self) -> Self {
        self.shared_memory = true;
        self
    }

    /// Opt-in (default off, so `eval` and `new Function` work): code generation from strings
    /// throws an `EvalError` in every context this engine creates.
    pub fn disallow_code_generation_from_strings(mut self) -> Self {
        self.disallow_codegen = true;
        self
    }

    /// The first isolate created gets a V8 inspector driven by `options.events` (VELYR-120).
    pub fn with_inspector(mut self, options: InspectorOptions) -> Self {
        self.inspector = Some(Arc::new(std::sync::Mutex::new(Some(options))));
        self
    }

    /// Initializes the process-wide V8 platform exactly once (ADR-010). Idempotent and
    /// thread-safe; `create_isolate` also calls it defensively.
    pub fn init_platform() {
        PLATFORM_INIT.call_once(|| {
            v8::V8::set_flags_from_string(V8_FLAGS);
            let platform = v8::new_custom_platform(0, false, false, platform::WakePlatform);
            v8::V8::initialize_platform(platform.make_shared());
            v8::V8::initialize();
        });
    }
}

impl Engine for V8Engine {
    type Isolate = V8Isolate;

    fn features(&self) -> EngineFeatures {
        EngineFeatures::MODULES
            | EngineFeatures::PROMISES
            | EngineFeatures::BIGINT
            | EngineFeatures::ARRAY_BUFFER
            | EngineFeatures::SERIALIZE
            | EngineFeatures::CONTINUATION_CONTEXT
            | EngineFeatures::EXPLICIT_RESOURCE_MANAGEMENT
    }

    fn create_isolate(&self, opts: IsolateOptions) -> Result<V8Isolate, EngineError> {
        Self::init_platform();
        let mut params = v8::CreateParams::default();
        if let Some(limit) = opts.heap_limit_bytes {
            params = params.heap_limits(0, limit);
        }
        let mut isolate = match self.snapshot {
            Some(blob) => snapshot::boot(params, blob),
            None => v8::Isolate::new(params),
        };
        let platform_queue = platform::QueueGuard(platform::key(&isolate));
        let waker = PlatformWaker::new();
        waker.register_current();
        platform::register(platform_queue.0, waker);
        let shared = Rc::new(Shared::default());
        if self.shared_memory {
            shared.slots.borrow_mut().insert(
                std::any::TypeId::of::<serialize::SharedMemoryAllowed>(),
                Box::new(serialize::SharedMemoryAllowed),
            );
        }
        // Always installed: without the callback V8 aborts the process at its default limit too.
        let heap_guard = Some(HeapGuard::install(
            &mut isolate,
            Arc::clone(&shared.heap_exhausted),
        ));
        // ADR-009 item 4: Velyr owns microtask timing.
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
        modules::install_callbacks(&mut isolate);
        isolate.set_promise_reject_callback(host::on_promise_reject);
        isolate.set_slot(SharedSlot(Rc::clone(&shared)));
        let context = {
            v8::scope!(let scope, &mut isolate);
            let context = v8::Context::new(scope, Default::default());
            if self.disallow_codegen {
                context.set_allow_generation_from_strings(false);
            }
            let relinked = match self.snapshot {
                Some(_) => {
                    let scope = &mut v8::ContextScope::new(scope, context);
                    snapshot::relink(scope, &shared, &self.snapshot_ops)
                }
                None => Ok(()),
            };
            relinked.map(|()| v8::Global::new(scope, context))
        };
        let context = match context {
            Ok(context) => context,
            Err(err) => {
                // Before `heap_guard`: V8 may call the heap callback until the isolate is gone.
                drop(isolate);
                return Err(err);
            }
        };
        let inspector = self
            .inspector
            .as_ref()
            .and_then(|pending| pending.lock().ok()?.take())
            .map(|options| inspector::Inspector::attach(&mut isolate, &context, options));
        Ok(V8Isolate {
            shared,
            context,
            inspector,
            isolate: RefCell::new(isolate),
            _heap_guard: heap_guard,
            platform_queue,
        })
    }

    fn boots_from_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }
}

/// One V8 isolate with a single context. `!Send`: must stay on its creating thread.
pub struct V8Isolate {
    shared: Rc<Shared>,
    context: v8::Global<v8::Context>,
    // Declared before `isolate`: the inspector must be gone before the isolate.
    inspector: Option<inspector::Inspector>,
    // Declared last so the `Global`s above are dropped while the isolate is alive.
    // `RefCell` only because `Isolate::heap_stats(&self)` needs `&mut v8::Isolate`.
    isolate: RefCell<v8::OwnedIsolate>,
    // Declared after `isolate`: V8 may call the heap callback until the isolate is gone.
    _heap_guard: Option<Box<HeapGuard>>,
    // Declared after `isolate`: drops the tasks V8 posted until the isolate was gone.
    platform_queue: platform::QueueGuard,
}

impl V8Isolate {
    /// Runs `f` inside the isolate's context. Cancels a termination that `f` observed so
    /// the isolate stays usable.
    fn enter<R>(&mut self, f: impl FnOnce(&mut v8::PinScope<'_, '_>, &Shared) -> R) -> R {
        let isolate = self.isolate.get_mut();
        let out = {
            v8::scope!(let handle_scope, &mut *isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let scope = &mut v8::ContextScope::new(handle_scope, context);
            f(scope, &self.shared)
        };
        if self.shared.terminated.take() {
            isolate.cancel_terminate_execution();
        }
        out
    }
}

/// Origin of a classic script: `name` is what its stack frames show.
fn script_origin<'s>(
    scope: &v8::PinScope<'s, '_>,
    name: v8::Local<'s, v8::String>,
) -> v8::ScriptOrigin<'s> {
    v8::ScriptOrigin::new(
        scope,
        name.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    )
}

impl Drop for V8Isolate {
    fn drop(&mut self) {
        // Node-API teardown runs addon finalizers in the context: before any Global is released.
        self.enter(host::run_native_teardown);
        // The isolate slot keeps `Shared` alive past this point; release its Globals first.
        self.shared.clear_globals();
    }
}

/// Compiles `source` as a classic script and, when `run`, runs it; `undefined` if not run.
pub(crate) fn script(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    source: &Source,
    run: bool,
) -> Result<Value, JsException> {
    v8::tc_scope!(let tc, scope);
    let (Some(name), Some(text)) = (
        v8::String::new(tc, &source.name),
        v8::String::new(tc, &source.text),
    ) else {
        return Err(internal_exception("source too long"));
    };
    let origin = script_origin(tc, name);
    let Some(compiled) = v8::Script::compile(tc, text, Some(&origin)) else {
        return Err(exception_from(tc, shared));
    };
    if !run {
        return Ok(Value::Undefined);
    }
    shared.microtasks_dirty.set(true);
    match compiled.run(tc) {
        Some(value) => Ok(from_v8(tc, &mut shared.handles.borrow_mut(), value)),
        None => Err(exception_from(tc, shared)),
    }
}

impl Isolate for V8Isolate {
    fn eval_script(&mut self, source: &Source) -> Result<Value, JsException> {
        self.enter(|scope, shared| script(scope, shared, source, true))
    }

    fn check_script(&mut self, source: &Source) -> Result<(), JsException> {
        self.enter(|scope, shared| script(scope, shared, source, false).map(drop))
    }

    fn compile_module(&mut self, source: &Source) -> Result<Handle, JsException> {
        self.enter(|scope, shared| modules::compile(scope, shared, source, None))
    }

    fn compile_module_cached(
        &mut self,
        source: &Source,
        cache: Option<&[u8]>,
    ) -> Result<Handle, JsException> {
        self.enter(|scope, shared| modules::compile(scope, shared, source, cache))
    }

    fn create_code_cache(&mut self, name: &str) -> Option<Vec<u8>> {
        self.enter(|scope, shared| code_cache::create(scope, shared, name))
    }

    fn compile_function(
        &mut self,
        source: &Source,
        params: &[&str],
        cache: Option<&[u8]>,
    ) -> Result<Value, JsException> {
        self.enter(|scope, shared| {
            code_cache::compile_function(scope, shared, source, params, cache)
        })
    }

    fn create_function_code_cache(&mut self, name: &str) -> Option<Vec<u8>> {
        self.enter(|scope, shared| code_cache::create_function(scope, shared, name))
    }

    fn code_cache_stats(&self) -> CodeCacheStats {
        self.shared.code_cache_stats.get()
    }

    fn instantiate_module(
        &mut self,
        module: Handle,
        hook: &mut dyn ModuleResolveHook,
    ) -> Result<(), JsException> {
        self.enter(|scope, shared| modules::instantiate(scope, shared, module, hook))
    }

    fn evaluate_module(&mut self, module: Handle) -> Result<Value, JsException> {
        if let Some(inspector) = &self.inspector {
            inspector.before_module_evaluation();
        }
        self.enter(|scope, shared| modules::evaluate(scope, shared, module))
    }

    fn call(&mut self, func: &Value, this: &Value, args: &[Value]) -> Result<Value, JsException> {
        self.enter(|scope, shared| {
            v8::tc_scope!(let tc, scope);
            let func = to_v8(tc, &shared.handles.borrow(), func)?;
            let func = v8::Local::<v8::Function>::try_from(func)
                .map_err(|_| internal_exception("not a function"))?;
            let this = to_v8(tc, &shared.handles.borrow(), this)?;
            let args = args
                .iter()
                .map(|a| to_v8(tc, &shared.handles.borrow(), a))
                .collect::<Result<Vec<_>, _>>()?;
            shared.microtasks_dirty.set(true);
            match func.call(tc, this, &args) {
                Some(value) => Ok(from_v8(tc, &mut shared.handles.borrow_mut(), value)),
                None => Err(exception_from(tc, shared)),
            }
        })
    }

    fn register_host_function(&mut self, def: HostFunctionDef) -> Result<(), EngineError> {
        self.enter(|scope, shared| host::install(scope, shared, def))?;
        Ok(())
    }

    fn set_slot<T: Any>(&mut self, value: T) {
        let slots = &mut self.shared.slots.borrow_mut();
        slots.insert(TypeId::of::<T>(), Box::new(value));
    }

    fn create_promise(&mut self) -> (Value, ResolverId) {
        self.enter(|scope, shared| shared.new_promise(scope))
    }

    fn resolve(&mut self, resolver: ResolverId, value: Value) {
        self.enter(|scope, shared| shared.settle(scope, resolver, &value, true));
    }

    fn reject(&mut self, resolver: ResolverId, reason: Value) {
        self.enter(|scope, shared| shared.settle(scope, resolver, &reason, false));
    }

    fn create_array_buffer(&mut self, bytes: &[u8]) -> Value {
        self.enter(|scope, shared| host::new_array_buffer(scope, shared, bytes))
    }

    fn create_velyr_error(&mut self, err: &VelyrError) -> Value {
        self.enter(|scope, shared| {
            let error = host::velyr_error_to_js(scope, err);
            from_v8(scope, &mut shared.handles.borrow_mut(), error)
        })
    }

    fn promise_state(&mut self, promise: &Value) -> Option<PromiseState> {
        self.enter(|scope, shared| host::promise_state(scope, shared, promise))
    }

    fn set_prepare_stack_trace(&mut self, mapper: velyr_engine_api::StackFrameMapper) {
        stack_trace::install(self.isolate.get_mut(), mapper);
    }

    fn set_entry_module(&mut self, name: &str) {
        *self.shared.entry_module.borrow_mut() = Some(name.to_owned());
    }

    fn set_continuation_context(&mut self, value: Value) {
        self.enter(|scope, shared| continuation::set(scope, shared, &value));
    }

    fn get_continuation_context(&mut self) -> Value {
        self.enter(continuation::get)
    }

    fn take_unhandled_rejections(&mut self) -> Vec<Value> {
        self.enter(|scope, shared| shared.take_unhandled(scope))
    }

    // shortcut: V8 exposes no queue length, so Progressed means JS ran (or a promise was
    // settled) since the last checkpoint; upgrade to a promise hook if the loop needs exactness.
    fn perform_microtask_checkpoint(&mut self) -> MicrotaskProgress {
        self.isolate.get_mut().perform_microtask_checkpoint();
        if self.shared.microtasks_dirty.take() {
            MicrotaskProgress::Progressed
        } else {
            MicrotaskProgress::Idle
        }
    }

    fn heap_stats(&self) -> HeapStats {
        let stats = self.isolate.borrow_mut().get_heap_statistics();
        HeapStats {
            used_bytes: stats.used_heap_size(),
            total_bytes: stats.total_heap_size(),
            limit_bytes: Some(stats.heap_size_limit()),
            external_bytes: Some(stats.external_memory()),
            // V8's HeapStatistics has no cell count or collection counters.
            ..HeapStats::default()
        }
    }

    fn request_gc(&mut self, _reason: GcReason) {
        self.isolate.get_mut().low_memory_notification();
    }

    // Background work ends with a foreground task posted to the queue, which wakes the loop.
    fn run_platform_tasks(&mut self) -> bool {
        if let Some(inspector) = &self.inspector {
            inspector.pump();
        }
        let key = self.platform_queue.0;
        let ran = platform::run_due(key);
        if ran {
            self.shared.microtasks_dirty.set(true);
        }
        // `ran`: the loop owes the tasks' microtasks a checkpoint. Background state before the
        // queue: work that ends right after still shows up as a due task.
        ran || self.isolate.get_mut().has_pending_background_tasks() || platform::has_due(key)
    }

    fn termination_handle(&self) -> TerminationHandle {
        let handle = self.isolate.borrow().thread_safe_handle();
        TerminationHandle::new(move || {
            handle.terminate_execution();
        })
    }

    fn serialize(
        &mut self,
        value: &Value,
        transfer: &[Value],
    ) -> Result<SerializedValue, JsException> {
        self.enter(|scope, shared| {
            v8::tc_scope!(let tc, scope);
            serialize::serialize(tc, shared, value, transfer)
                .ok_or_else(|| exception_from(tc, shared))
        })
    }

    fn deserialize(&mut self, serialized: &SerializedValue) -> Result<Value, JsException> {
        self.enter(|scope, shared| {
            v8::tc_scope!(let tc, scope);
            serialize::deserialize(tc, shared, serialized).ok_or_else(|| exception_from(tc, shared))
        })
    }

    fn release(&mut self, handle: Handle) {
        self.shared.handles.borrow_mut().release(handle);
    }

    fn scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.shared.handles.borrow_mut().push_scope();
        let out = f(self);
        self.shared.handles.borrow_mut().pop_scope();
        out
    }
}

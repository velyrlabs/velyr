//! V8 adapter for `velyr-engine-api` (ADR-009). No `v8::` type appears in the public API.

mod code_cache;
mod convert;
mod handles;
mod heap;
mod host;
mod modules;
mod serialize;
mod shared;

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Once};

use velyr_engine_api::{
    CodeCacheStats, Engine, EngineError, EngineFeatures, GcReason, Handle, HeapStats,
    HostFunctionDef, Isolate, IsolateOptions, JsException, MicrotaskProgress, ModuleResolveHook,
    PromiseState, ResolverId, SerializedValue, Source, TerminationHandle, Value,
};
use velyr_runtime_errors::VelyrError;

use convert::{exception_from, from_v8, internal_exception, to_v8};
use heap::HeapGuard;
use host::SharedSlot;
use shared::Shared;

static PLATFORM_INIT: Once = Once::new();

/// Factory for [`V8Isolate`]s.
#[derive(Debug, Default, Clone, Copy)]
pub struct V8Engine;

impl V8Engine {
    pub fn new() -> Self {
        Self
    }

    /// Initializes the process-wide V8 platform exactly once (ADR-010). Idempotent and
    /// thread-safe; `create_isolate` also calls it defensively.
    pub fn init_platform() {
        PLATFORM_INIT.call_once(|| {
            v8::V8::initialize_platform(v8::new_default_platform(0, false).make_shared());
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
    }

    fn create_isolate(&self, opts: IsolateOptions) -> Result<V8Isolate, EngineError> {
        Self::init_platform();
        let mut params = v8::CreateParams::default();
        if let Some(limit) = opts.heap_limit_bytes {
            params = params.heap_limits(0, limit);
        }
        let mut isolate = v8::Isolate::new(params);
        let shared = Rc::new(Shared::default());
        let heap_guard = opts
            .heap_limit_bytes
            .map(|_| HeapGuard::install(&mut isolate, Arc::clone(&shared.heap_exhausted)));
        // ADR-009 item 4: Velyr owns microtask timing.
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
        modules::install_callbacks(&mut isolate);
        isolate.set_promise_reject_callback(host::on_promise_reject);
        isolate.set_slot(SharedSlot(Rc::clone(&shared)));
        let context = {
            v8::scope!(let scope, &mut isolate);
            let context = v8::Context::new(scope, Default::default());
            v8::Global::new(scope, context)
        };
        Ok(V8Isolate {
            shared,
            context,
            isolate: RefCell::new(isolate),
            _heap_guard: heap_guard,
        })
    }
}

/// One V8 isolate with a single context. `!Send`: must stay on its creating thread.
pub struct V8Isolate {
    shared: Rc<Shared>,
    context: v8::Global<v8::Context>,
    // Declared last so the `Global`s above are dropped while the isolate is alive.
    // `RefCell` only because `Isolate::heap_stats(&self)` needs `&mut v8::Isolate`.
    isolate: RefCell<v8::OwnedIsolate>,
    // Declared after `isolate`: V8 may call the heap callback until the isolate is gone.
    _heap_guard: Option<Box<HeapGuard>>,
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

impl Drop for V8Isolate {
    fn drop(&mut self) {
        // The isolate slot keeps `Shared` alive past this point; release its Globals first.
        self.shared.clear_globals();
    }
}

impl Isolate for V8Isolate {
    fn eval_script(&mut self, source: &Source) -> Result<Value, JsException> {
        self.enter(|scope, shared| {
            v8::tc_scope!(let tc, scope);
            let (Some(name), Some(text)) = (
                v8::String::new(tc, &source.name),
                v8::String::new(tc, &source.text),
            ) else {
                return Err(internal_exception("source too long"));
            };
            let origin = v8::ScriptOrigin::new(
                tc,
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
            );
            shared.microtasks_dirty.set(true);
            let result = v8::Script::compile(tc, text, Some(&origin)).and_then(|s| s.run(tc));
            match result {
                Some(value) => Ok(from_v8(tc, &mut shared.handles.borrow_mut(), value)),
                None => Err(exception_from(tc, shared)),
            }
        })
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
        }
    }

    fn request_gc(&mut self, _reason: GcReason) {
        self.isolate.get_mut().low_memory_notification();
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

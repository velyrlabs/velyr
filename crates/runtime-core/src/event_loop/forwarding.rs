//! `Isolate` for [`LoopIsolate`]: every method forwards to the engine, asserting the owner
//! thread; JS-running calls are timed as turns. A method left to its trait default would drop the
//! call before it reaches the engine.

use std::any::Any;

use velyr_engine_api::{
    CodeCacheStats, EngineError, GcReason, Handle, HeapStats, HostFunctionDef, Isolate,
    JsException, MicrotaskProgress, ModuleResolveHook, PromiseState, ResolverId, SharedSource,
    Source, TerminationHandle, Value,
};
use velyr_runtime_errors::VelyrError;

use super::LoopIsolate;

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

    fn compile_module_shared(
        &mut self,
        source: &SharedSource,
        cache: Option<&[u8]>,
    ) -> Result<Handle, JsException> {
        self.on_owner();
        self.inner.compile_module_shared(source, cache)
    }

    fn create_code_cache(&mut self, name: &str) -> Option<Vec<u8>> {
        self.on_owner();
        self.inner.create_code_cache(name)
    }

    fn compile_function(
        &mut self,
        source: &Source,
        params: &[&str],
        cache: Option<&[u8]>,
    ) -> Result<Value, JsException> {
        self.on_owner();
        self.inner.compile_function(source, params, cache)
    }

    fn create_function_code_cache(&mut self, name: &str) -> Option<Vec<u8>> {
        self.on_owner();
        self.inner.create_function_code_cache(name)
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

    fn check_script(&mut self, source: &Source) -> Result<(), JsException> {
        self.on_owner();
        self.inner.check_script(source)
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
        let flags = self.tick_flags.clone();
        self.turn(|inner| {
            // `ran` can't come from the engine's progress flag: calling the hook marks it dirty.
            let call_hook = |inner: &mut I| {
                if flags.armed() && !flags.has_tick() {
                    return false;
                }
                match inner.call(&hook, &Value::Undefined, &[]) {
                    Ok(value) => value == Value::Bool(true),
                    Err(err) => {
                        tracing::warn!(message = %err.message, "tick hook threw");
                        false
                    }
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

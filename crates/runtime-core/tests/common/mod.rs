//! Minimal mock engine: evaluates numeric literals only.
#![allow(dead_code)]

use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};

use velyr_engine_api::*;

#[derive(Default)]
pub struct MockEngine;

pub struct MockIsolate;

/// Calls that reached `MockIsolate::create_array_buffer_owned`.
pub static OWNED_CALLS: AtomicUsize = AtomicUsize::new(0);

impl Engine for MockEngine {
    type Isolate = MockIsolate;

    fn features(&self) -> EngineFeatures {
        EngineFeatures::empty()
    }

    fn create_isolate(&self, _opts: IsolateOptions) -> Result<MockIsolate, EngineError> {
        Ok(MockIsolate)
    }
}

impl Isolate for MockIsolate {
    fn eval_script(&mut self, source: &Source) -> Result<Value, JsException> {
        source
            .text
            .trim()
            .parse::<f64>()
            .map(Value::Number)
            .map_err(|_| JsException {
                message: format!("SyntaxError: {}", source.text),
                stack: None,
                value: None,
            })
    }

    fn compile_module(&mut self, _: &Source) -> Result<Handle, JsException> {
        unimplemented!()
    }
    fn instantiate_module(
        &mut self,
        _: Handle,
        _: &mut dyn ModuleResolveHook,
    ) -> Result<(), JsException> {
        unimplemented!()
    }
    fn evaluate_module(&mut self, _: Handle) -> Result<Value, JsException> {
        unimplemented!()
    }
    fn call(&mut self, _: &Value, _: &Value, _: &[Value]) -> Result<Value, JsException> {
        unimplemented!()
    }
    fn register_host_function(&mut self, _: HostFunctionDef) -> Result<(), EngineError> {
        unimplemented!()
    }
    fn set_slot<T: Any>(&mut self, _: T) {}
    fn create_promise(&mut self) -> (Value, ResolverId) {
        unimplemented!()
    }
    fn resolve(&mut self, _: ResolverId, _: Value) {}
    fn reject(&mut self, _: ResolverId, _: Value) {}
    fn create_array_buffer_owned(&mut self, _: Vec<u8>) -> Value {
        OWNED_CALLS.fetch_add(1, Ordering::SeqCst);
        Value::Undefined
    }
    fn promise_state(&mut self, _: &Value) -> Option<PromiseState> {
        None
    }
    fn perform_microtask_checkpoint(&mut self) -> MicrotaskProgress {
        MicrotaskProgress::Idle
    }
    fn heap_stats(&self) -> HeapStats {
        HeapStats::default()
    }
    fn request_gc(&mut self, _: GcReason) {}
    fn termination_handle(&self) -> TerminationHandle {
        TerminationHandle::new(|| {})
    }
    fn release(&mut self, _: Handle) {}
    fn scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        f(self)
    }
}

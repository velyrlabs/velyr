//! `NativeIsolate`: scripts compile to register bytecode and run on the `velyr-engine-vm`
//! interpreter; host functions become native function objects of its realm.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use velyr_engine_api::*;
use velyr_engine_vm::Value as Val;
use velyr_engine_vm::interpreter::{
    Abrupt, CompileGoal, ErrorKind, Interpreter, NativeFn, PromiseStatus,
};
use velyr_runtime_errors::{ErrorCode, VelyrError};

use crate::bytecode_compiler::{compile_indirect_eval, compile_named_script};
use crate::compiler::{JsResult, compile, throw};
use crate::modules::{DYNAMIC_IMPORT_OP, ModuleGraph};
use crate::value_bridge::{export, import};
use crate::vm::{Slots, State};

/// `!Send` because the VM state is built on `Rc`.
pub struct NativeIsolate {
    pub(crate) vm: Interpreter,
    /// Tree interpreter kept only for the `unsupported` fallback in `eval_script`.
    tree: State,
    slots: Rc<RefCell<Slots>>,
    terminated: Arc<AtomicBool>,
    pub(crate) modules: ModuleGraph,
}

impl NativeIsolate {
    pub(crate) fn new() -> Self {
        let mut vm = Interpreter::new();
        // `Function(...)` and indirect `eval` compile their source text through this hook.
        vm.set_compile_hook(Rc::new(|source: &str, goal| {
            let code = match goal {
                CompileGoal::Function => compile_named_script("<anonymous>", source),
                CompileGoal::IndirectEval => compile_indirect_eval(source),
            };
            code.map_err(|error| error.message)
        }));
        crate::iterator_helpers::install(&mut vm).expect("the iterator helpers prelude runs");
        crate::prelude::install_lazy_stubs(&mut vm).expect("the lazy stubs prelude runs");
        crate::array_from_async::install(&mut vm).expect("the Array.fromAsync prelude runs");
        crate::disposable::install(&mut vm).expect("the disposable prelude runs");
        crate::async_disposable::install(&mut vm).expect("the async disposable prelude runs");
        crate::date::install(&mut vm).expect("the Date prelude runs");
        crate::intl::install(&mut vm).expect("the Intl prelude runs");
        crate::temporal::install(&mut vm).expect("the Temporal prelude runs");
        let terminated = vm.termination_flag();
        Self {
            vm,
            tree: State::new(),
            slots: Rc::new(RefCell::new(Slots::new())),
            terminated,
            modules: ModuleGraph::default(),
        }
    }

    /// Globals still holding an unmaterialized lazy namespace shell, in definition order.
    #[doc(hidden)]
    pub fn pending_lazy_globals(&mut self) -> Vec<String> {
        self.vm.pending_lazy()
    }

    /// Maps an abrupt completion of the interpreter onto the engine-api exception.
    pub(crate) fn exception(&mut self, abrupt: Abrupt) -> JsException {
        match abrupt {
            Abrupt::Throw(value) => JsException {
                message: self.vm.exception_message(&value),
                stack: self.vm.exception_stack(&value),
                value: export_value(&mut self.vm, &value).ok(),
            },
            Abrupt::Terminated => self.terminated_exception(),
            Abrupt::Heap(error) if error.code() == ErrorCode::QuotaExceeded => {
                JsException::quota_exceeded("heap")
            }
            Abrupt::Heap(error) => throw("Error", format!("{}: {}", error.code(), error.message())),
        }
    }

    /// The termination was observed and is being reported: clear the flag so the isolate
    /// can run again, like engine-v8's `cancel_terminate_execution`. Same message as
    /// engine-v8, which runtime-core uses to detect termination and the CPU budget.
    fn terminated_exception(&mut self) -> JsException {
        self.terminated.store(false, Ordering::Relaxed);
        JsException {
            message: "execution terminated".into(),
            stack: None,
            value: None,
        }
    }

    fn settle(&mut self, resolver: ResolverId, value: &Value, rejects: bool) {
        // Values that cannot cross (stale handles) and abrupt settlements are dropped:
        // the trait has no error channel.
        if self.settle_dynamic_import(resolver.0, value, rejects) {
            return;
        }
        if let Ok(value) = import_value(&self.vm, value)
            && let Err(abrupt) = self.vm.settle_promise(resolver.0, &value, rejects)
        {
            let _ = self.exception(abrupt);
        }
    }

    pub(crate) fn complete(&mut self, result: Result<Val, Abrupt>) -> Result<Value, JsException> {
        let value = result.map_err(|abrupt| self.exception(abrupt))?;
        export_value(&mut self.vm, &value)
    }

    /// shortcut: fallback to tree interpreter for constructs the bytecode compiler does
    /// not support yet, remove when compiler covers them.
    fn eval_on_tree(&mut self, source: &Source) -> Result<Value, JsException> {
        let ops = compile(&source.text)?;
        self.tree.slots = std::mem::take(&mut *self.slots.borrow_mut());
        let completion = self.tree.run(&ops, &self.terminated);
        *self.slots.borrow_mut() = std::mem::take(&mut self.tree.slots);
        if completion.is_err() && self.terminated.load(Ordering::Relaxed) {
            return Err(self.terminated_exception());
        }
        export(&completion?)
    }
}

/// Engine value -> engine-api value; objects get a rooted handle (see `Isolate::release`).
fn export_value(vm: &mut Interpreter, value: &Val) -> JsResult<Value> {
    match vm.create_handle(value) {
        Some(handle) => Ok(Value::Object(handle)),
        None => export(value),
    }
}

/// Engine-api value -> engine value; object handles must be live in this isolate.
fn import_value(vm: &Interpreter, value: &Value) -> JsResult<Val> {
    match value {
        Value::Object(handle) => vm
            .resolve_handle(*handle)
            .ok_or_else(|| throw("TypeError", "object handle is not live in this isolate")),
        other => Ok(import(other)),
    }
}

fn is_unsupported(error: &JsException) -> bool {
    error.message.starts_with("SyntaxError: unsupported: ")
}

/// Native function body calling the host callback with engine-api values.
fn host_function(def: HostFunctionDef, slots: Rc<RefCell<Slots>>) -> NativeFn {
    Rc::new(move |vm, _this, args| {
        // Argument handles are valid only for the duration of the call (HostFn contract).
        vm.push_handle_scope();
        let exported = args
            .iter()
            .map(|arg| export_value(vm, arg))
            .collect::<JsResult<Vec<_>>>();
        let result = exported.map(|exported| {
            let mut slots = slots.borrow_mut();
            (def.callback)(&mut HostContext(&mut slots, vm), &exported)
        });
        // Handles (e.g. a thrown object) are only live inside the scope: import before it pops.
        let outcome = match result {
            Ok(Ok(value)) => import_value(vm, &value)
                .map_err(|error| vm.error(ErrorKind::Type, strip_kind(&error.message))),
            Ok(Err(HostError::TypeError(message))) => Err(vm.error(ErrorKind::Type, &message)),
            Ok(Err(HostError::Thrown(value))) => Err(match import_value(vm, &value) {
                Ok(value) => Abrupt::Throw(value),
                Err(error) => vm.error(ErrorKind::Type, strip_kind(&error.message)),
            }),
            // Same shape as engine-v8: the message, with the code on `error.code`.
            Ok(Err(HostError::Velyr(error))) => {
                Err(vm.error_with_code(error.message(), error.code().as_str()))
            }
            Err(error) => Err(vm.error(ErrorKind::Type, strip_kind(&error.message))),
        };
        vm.pop_handle_scope();
        outcome
    })
}

/// A nested script's abrupt completion as the error a host function returns (inverse of the
/// HostError -> Abrupt mapping in the host-call trampoline above).
fn abrupt_to_host(vm: &mut Interpreter, abrupt: Abrupt) -> HostError {
    match abrupt {
        Abrupt::Throw(value) => match export_value(vm, &value) {
            Ok(value) => HostError::Thrown(value),
            Err(error) => HostError::TypeError(error.message),
        },
        // The termination flag stays raised (only `NativeIsolate::exception` clears it), so the
        // outer run stops at its next poll; same shape as engine-v8 `exception_to_host`.
        Abrupt::Terminated => HostError::Velyr(VelyrError::invalid_data("execution terminated")),
        Abrupt::Heap(error) => HostError::Velyr(error),
    }
}

fn strip_kind(message: &str) -> &str {
    message.split_once(": ").map_or(message, |(_, rest)| rest)
}

impl Isolate for NativeIsolate {
    fn set_prepare_stack_trace(&mut self, mapper: velyr_engine_api::StackFrameMapper) {
        self.vm
            .set_frame_mapper(Some(Arc::new(move |file, line, column| {
                mapper(file, line, column).map(|m| (m.file, m.line, m.column))
            })));
    }

    fn eval_script(&mut self, source: &Source) -> Result<Value, JsException> {
        // Compiling interns atoms, which this isolate's heap must be charged for.
        self.vm.enter_heap();
        match compile_named_script(&source.name, &source.text) {
            Ok(code) => {
                let result = self.vm.run_script(&code);
                self.complete(result)
            }
            Err(error) if is_unsupported(&error) => self.eval_on_tree(source).map_err(|_| error),
            Err(error) => Err(error),
        }
    }

    fn compile_module(&mut self, source: &Source) -> Result<Handle, JsException> {
        self.vm.enter_heap();
        self.compile_module_source(source)
    }

    fn instantiate_module(
        &mut self,
        module: Handle,
        hook: &mut dyn ModuleResolveHook,
    ) -> Result<(), JsException> {
        self.instantiate(module, hook)
    }

    fn evaluate_module(&mut self, module: Handle) -> Result<Value, JsException> {
        self.vm.enter_heap();
        self.evaluate(module)
    }

    fn call(&mut self, func: &Value, this: &Value, args: &[Value]) -> Result<Value, JsException> {
        self.vm.enter_heap();
        let func = import_value(&self.vm, func)?;
        let this = import_value(&self.vm, this)?;
        let args = args
            .iter()
            .map(|arg| import_value(&self.vm, arg))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self.vm.call(&func, &this, &args);
        self.complete(result)
    }

    fn register_host_function(&mut self, def: HostFunctionDef) -> Result<(), EngineError> {
        self.tree.register_host(def)?;
        let function = host_function(def, self.slots.clone());
        if def.name == DYNAMIC_IMPORT_OP {
            self.install_dynamic_import(function.clone());
        }
        self.vm
            .define_global_function(def.name, u32::from(def.arity), function)
            .map_err(|_| {
                VelyrError::internal(format!("cannot define host function {}", def.name))
            })?;
        Ok(())
    }

    fn set_slot<T: Any>(&mut self, value: T) {
        self.slots
            .borrow_mut()
            .insert(TypeId::of::<T>(), Box::new(value));
    }

    fn create_promise(&mut self) -> (Value, ResolverId) {
        let created = self
            .vm
            .create_promise()
            .map_err(drop)
            .and_then(|(promise, id)| {
                let promise = export_value(&mut self.vm, &promise).map_err(drop)?;
                Ok((promise, ResolverId(id)))
            });
        // shortcut: the trait is infallible, so a heap failure yields an inert promise.
        created.unwrap_or((Value::Undefined, ResolverId(u64::MAX)))
    }

    fn resolve(&mut self, resolver: ResolverId, value: Value) {
        self.settle(resolver, &value, false);
    }

    fn reject(&mut self, resolver: ResolverId, reason: Value) {
        self.settle(resolver, &reason, true);
    }

    fn create_array_buffer(&mut self, bytes: &[u8]) -> Value {
        HostContext(&mut self.slots.borrow_mut(), &mut self.vm).create_array_buffer(bytes)
    }

    fn set_continuation_context(&mut self, value: Value) {
        HostContext(&mut self.slots.borrow_mut(), &mut self.vm).set_continuation_context(value);
    }

    fn get_continuation_context(&mut self) -> Value {
        HostContext(&mut self.slots.borrow_mut(), &mut self.vm).get_continuation_context()
    }

    fn promise_state(&mut self, promise: &Value) -> Option<PromiseState> {
        let promise = import_value(&self.vm, promise).ok()?;
        let export = |vm: &mut Interpreter, value: Val| export_value(vm, &value).ok();
        Some(match self.vm.promise_status(&promise)? {
            PromiseStatus::Pending => PromiseState::Pending,
            PromiseStatus::Fulfilled(value) => {
                PromiseState::Fulfilled(export(&mut self.vm, value)?)
            }
            PromiseStatus::Rejected(value) => PromiseState::Rejected(export(&mut self.vm, value)?),
        })
    }

    fn perform_microtask_checkpoint(&mut self) -> MicrotaskProgress {
        self.vm.enter_heap();
        match self.vm.perform_microtask_checkpoint() {
            Ok(false) => MicrotaskProgress::Idle,
            Ok(true) => MicrotaskProgress::Progressed,
            Err(abrupt) => {
                // Report-and-clear like `exception`, so the isolate can run again.
                let _ = self.exception(abrupt);
                MicrotaskProgress::Progressed
            }
        }
    }

    fn take_unhandled_rejections(&mut self) -> Vec<Value> {
        let reasons = self.vm.unhandled_rejections();
        reasons
            .iter()
            .filter_map(|reason| export_value(&mut self.vm, reason).ok())
            .collect()
    }

    fn heap_stats(&self) -> HeapStats {
        let (used_bytes, limit_bytes) = self.vm.heap_usage();
        let gc = self.vm.gc_stats();
        HeapStats {
            used_bytes,
            total_bytes: used_bytes,
            limit_bytes,
            external_bytes: Some(gc.external_bytes),
            cells: Some(gc.live_cells),
            gc_count: Some(gc.collections),
            gc_time: Some(gc.total_pause),
            breakdown: Some(MemoryBreakdown {
                cells: gc.live_bytes,
                heap_tables: gc.table_bytes,
                strings: gc.breakdown.strings,
                storage: gc.breakdown.storage,
                code: gc.breakdown.code,
                source: gc.breakdown.source,
                shapes: gc.breakdown.shapes,
                interner: gc.breakdown.interner,
                tables: gc.breakdown.tables,
            }),
        }
    }

    fn request_gc(&mut self, _reason: GcReason) {
        self.vm.collect_garbage();
    }

    fn termination_handle(&self) -> TerminationHandle {
        let flag = self.terminated.clone();
        TerminationHandle::new(move || flag.store(true, Ordering::Relaxed))
    }

    fn release(&mut self, handle: Handle) {
        self.vm.release_handle(handle);
    }

    fn set_entry_module(&mut self, name: &str) {
        self.vm.set_entry_module(name);
    }

    fn check_script(&mut self, source: &Source) -> Result<(), JsException> {
        self.vm.enter_heap();
        compile_named_script(&source.name, &source.text).map(drop)
    }

    fn scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.vm.push_handle_scope();
        let result = f(self);
        self.vm.pop_handle_scope();
        result
    }
}

/// `CallContext` for host functions: strings, arrays, slots and `TypeError`s.
/// shortcut: plain objects are not exposed to hosts yet; the
/// infallible constructors return `undefined`, the fallible ones `VELYR_UNSUPPORTED`.
struct HostContext<'a>(&'a mut Slots, &'a mut Interpreter);

fn unsupported_host(what: &str) -> HostError {
    HostError::Velyr(VelyrError::unsupported(format!(
        "{what} is not supported by engine-native bootstrap"
    )))
}

impl CallContext for HostContext<'_> {
    fn eval_script(&mut self, source: &Source) -> Result<Value, HostError> {
        let vm = &mut *self.1;
        let code = match compile_named_script(&source.name, &source.text) {
            Ok(code) => code,
            Err(error) => {
                let abrupt = vm.error(ErrorKind::Syntax, strip_kind(&error.message));
                return Err(abrupt_to_host(vm, abrupt));
            }
        };
        let value = vm
            .run_script(&code)
            .map_err(|abrupt| abrupt_to_host(vm, abrupt))?;
        export_value(vm, &value).map_err(|error| HostError::TypeError(error.message))
    }
    fn request_gc(&mut self) -> Result<(), HostError> {
        self.1.collect_garbage();
        Ok(())
    }
    fn create_string(&mut self, value: JsString) -> Value {
        Value::String(value)
    }
    fn create_object(&mut self) -> Value {
        Value::Undefined
    }
    /// `undefined` when an item is not live here or the heap quota is hit.
    fn create_array(&mut self, items: &[Value]) -> Value {
        let vm = &mut *self.1;
        let items = items.iter().map(|item| import_value(vm, item));
        let array = items
            .collect::<JsResult<Vec<_>>>()
            .ok()
            .and_then(|items| vm.create_array(&items).ok());
        array
            .and_then(|array| export_value(vm, &array).ok())
            .unwrap_or(Value::Undefined)
    }
    /// `undefined` when the heap quota is hit.
    fn create_array_buffer(&mut self, bytes: &[u8]) -> Value {
        let vm = &mut *self.1;
        vm.create_array_buffer(bytes)
            .ok()
            .and_then(|buffer| export_value(vm, &buffer).ok())
            .unwrap_or(Value::Undefined)
    }
    fn read_bytes(&mut self, value: &Value) -> Option<Vec<u8>> {
        let value = import_value(self.1, value).ok()?;
        self.1.with_buffer_bytes(&value, <[u8]>::to_vec)
    }
    fn get_property(&mut self, _object: &Value, _key: &str) -> Result<Value, HostError> {
        Err(unsupported_host("property access"))
    }
    fn set_property(
        &mut self,
        _object: &Value,
        _key: &str,
        _value: Value,
    ) -> Result<(), HostError> {
        Err(unsupported_host("property access"))
    }
    /// Async ops return this promise and settle it later through `Isolate::resolve`.
    fn create_promise(&mut self) -> (Value, ResolverId) {
        let vm = &mut *self.1;
        let created = vm.create_promise().ok().and_then(|(promise, id)| {
            let promise = export_value(vm, &promise).ok()?;
            Some((promise, ResolverId(id)))
        });
        // shortcut: the trait is infallible, so a heap failure yields an inert promise.
        created.unwrap_or((Value::Undefined, ResolverId(u64::MAX)))
    }
    // shortcut: no op settles inside its own call; they settle through `Isolate::resolve`.
    fn resolve(&mut self, _resolver: ResolverId, _value: Value) {}
    fn reject(&mut self, _resolver: ResolverId, _reason: Value) {}
    fn throw_type_error(&mut self, message: &str) -> HostError {
        HostError::TypeError(message.into())
    }
    /// A stale handle leaves the context as it was, as on engine-v8.
    fn set_continuation_context(&mut self, value: Value) {
        if let Ok(value) = import_value(self.1, &value) {
            self.1.set_continuation_context(value);
        }
    }
    /// `undefined` when the heap quota is hit.
    fn get_continuation_context(&mut self) -> Value {
        let vm = &mut *self.1;
        let context = vm.continuation_context();
        export_value(vm, &context).unwrap_or(Value::Undefined)
    }
    fn slot(&mut self, type_id: TypeId) -> Option<&mut dyn Any> {
        self.0.get_mut(&type_id).map(|slot| &mut **slot)
    }
}

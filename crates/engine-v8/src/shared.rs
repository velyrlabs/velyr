//! Isolate-wide state shared between `V8Isolate` and the host-function callbacks.
//!
//! Every field has its own cell and no borrow is held while JS runs, so a host function
//! re-entering the engine never trips a `BorrowMutError`.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use velyr_engine_api::{CodeCacheStats, HostFn, ResolverId, Value};

use crate::code_cache::CompiledFunction;
use crate::convert::{from_v8, to_v8};
use crate::handles::HandleTable;
use crate::modules::{self, ModuleGraph};

#[derive(Default)]
pub(crate) struct Shared {
    pub(crate) handles: RefCell<HandleTable>,
    pub(crate) modules: RefCell<ModuleGraph>,
    /// Functions from `compile_function` by `Source::name`, for `create_function_code_cache`.
    pub(crate) functions: RefCell<HashMap<String, CompiledFunction>>,
    /// Compile name of the entry module, for `import.meta.main`.
    pub(crate) entry_module: RefCell<Option<String>>,
    pub(crate) slots: RefCell<HashMap<TypeId, Box<dyn Any>>>,
    pub(crate) host_fns: RefCell<Vec<HostFn>>,
    /// Registered host function name -> index into `host_fns`.
    pub(crate) host_names: RefCell<HashMap<&'static str, usize>>,
    /// Resolvers created by `import()`: settling one with a module name finishes the import.
    dynamic_imports: RefCell<HashSet<u64>>,
    resolvers: RefCell<HashMap<u64, v8::Global<v8::PromiseResolver>>>,
    next_resolver: Cell<u64>,
    /// JS may have queued microtasks since the last checkpoint.
    pub(crate) microtasks_dirty: Cell<bool>,
    pub(crate) terminated: Cell<bool>,
    pub(crate) code_cache_stats: Cell<CodeCacheStats>,
    /// Set by the near-heap-limit callback (any thread order is fine: same isolate thread).
    pub(crate) heap_exhausted: Arc<AtomicBool>,
    /// Promises rejected with no handler yet, in rejection order.
    unhandled: RefCell<Vec<v8::Global<v8::Promise>>>,
}

impl Shared {
    /// Drops every `v8::Global` so none outlives the isolate.
    pub(crate) fn clear_globals(&self) {
        *self.handles.borrow_mut() = HandleTable::default();
        self.resolvers.borrow_mut().clear();
        self.unhandled.borrow_mut().clear();
        *self.modules.borrow_mut() = ModuleGraph::default();
        self.functions.borrow_mut().clear();
    }

    pub(crate) fn new_promise(&self, scope: &mut v8::PinScope<'_, '_>) -> (Value, ResolverId) {
        let resolver = v8::PromiseResolver::new(scope).expect("PromiseResolver::new failed");
        let promise: v8::Local<v8::Value> = resolver.get_promise(scope).into();
        let id = self.register_resolver(scope, resolver);
        let value = from_v8(scope, &mut self.handles.borrow_mut(), promise);
        (value, id)
    }

    pub(crate) fn register_resolver(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        resolver: v8::Local<v8::PromiseResolver>,
    ) -> ResolverId {
        let id = self.next_resolver.get();
        self.next_resolver.set(id + 1);
        self.resolvers
            .borrow_mut()
            .insert(id, v8::Global::new(scope, resolver));
        ResolverId(id)
    }

    /// Registers the resolver of an `import()` call; see [`Shared::settle`].
    pub(crate) fn register_dynamic_import(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        resolver: v8::Local<v8::PromiseResolver>,
    ) -> ResolverId {
        let id = self.register_resolver(scope, resolver);
        self.dynamic_imports.borrow_mut().insert(id.0);
        id
    }

    /// Drops a resolver the caller settled itself.
    pub(crate) fn forget_resolver(&self, id: ResolverId) {
        self.resolvers.borrow_mut().remove(&id.0);
        self.dynamic_imports.borrow_mut().remove(&id.0);
    }

    /// Settling twice, or with a stale handle, is a no-op. Resolving an `import()` resolver with
    /// `Value::String(module name)` evaluates that (instantiated) module and resolves the
    /// import with its namespace once evaluation, including top-level await, finishes.
    pub(crate) fn settle(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        id: ResolverId,
        value: &Value,
        resolve: bool,
    ) {
        let Some(resolver) = self.resolvers.borrow_mut().remove(&id.0) else {
            return;
        };
        let dynamic = self.dynamic_imports.borrow_mut().remove(&id.0);
        if let (true, true, Value::String(name)) = (dynamic, resolve, value) {
            let resolver = v8::Local::new(scope, &resolver);
            modules::finish_dynamic_import(scope, self, resolver, &name.to_string_lossy());
            self.microtasks_dirty.set(true);
            return;
        }
        let Ok(value) = to_v8(scope, &self.handles.borrow(), value) else {
            return;
        };
        let resolver = v8::Local::new(scope, &resolver);
        // Only fails if JS threw or was terminated; nothing to report on this path.
        let _ = if resolve {
            resolver.resolve(scope, value)
        } else {
            resolver.reject(scope, value)
        };
        self.microtasks_dirty.set(true);
    }
}

impl Shared {
    pub(crate) fn track_rejection(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        promise: v8::Local<v8::Promise>,
        event: v8::PromiseRejectEvent,
    ) {
        use v8::PromiseRejectEvent::{PromiseHandlerAddedAfterReject, PromiseRejectWithNoHandler};
        let mut unhandled = self.unhandled.borrow_mut();
        match event {
            PromiseRejectWithNoHandler => unhandled.push(v8::Global::new(scope, promise)),
            PromiseHandlerAddedAfterReject => {
                unhandled.retain(|tracked| v8::Local::new(scope, tracked) != promise);
            }
            _ => {}
        }
    }

    /// Marks `promise` as handled and drops it from the unhandled list.
    pub(crate) fn mark_handled(&self, scope: &mut v8::PinScope<'_, '_>, promise: &Value) {
        let Ok(value) = to_v8(scope, &self.handles.borrow(), promise) else {
            return;
        };
        let Ok(promise) = v8::Local::<v8::Promise>::try_from(value) else {
            return;
        };
        promise.mark_as_handled();
        self.unhandled
            .borrow_mut()
            .retain(|tracked| v8::Local::new(scope, tracked) != promise);
    }

    pub(crate) fn take_unhandled(&self, scope: &mut v8::PinScope<'_, '_>) -> Vec<Value> {
        let tracked: Vec<_> = self.unhandled.borrow().iter().cloned().collect();
        tracked
            .iter()
            .map(|promise| {
                let reason = v8::Local::new(scope, promise).result(scope);
                from_v8(scope, &mut self.handles.borrow_mut(), reason)
            })
            .collect()
    }
}

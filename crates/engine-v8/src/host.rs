//! Host functions: the V8 trampoline and the concrete [`CallContext`].

use std::any::{Any, TypeId};
use std::cell::RefMut;
use std::collections::HashMap;
use std::rc::Rc;

use velyr_engine_api::{
    CallContext, Handle, HostError, HostFn, HostFunctionDef, JsException, JsString,
    ModuleResolveHook, PromiseState, ResolverId, SerializedValue, Source, Value,
};
use velyr_runtime_errors::VelyrError;

use crate::convert::{from_v8, to_v8};
use crate::shared::Shared;
use crate::{modules, serialize};

/// Isolate-slot wrapper that lets callbacks find the [`Shared`] state.
pub(crate) struct SharedSlot(pub(crate) Rc<Shared>);

/// Installs `def` on the global object. `scope` must be inside the isolate's context.
pub(crate) fn install(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    def: HostFunctionDef,
) -> Result<(), VelyrError> {
    if shared.host_names.borrow().contains_key(def.name) {
        return Err(VelyrError::already_exists(format!(
            "host function `{}` is already registered",
            def.name
        )));
    }
    let internal = |what: &str| VelyrError::internal(format!("{what} `{}`", def.name));
    // The callback is addressed by index; no raw pointer crosses the FFI boundary.
    let index = shared.host_fns.borrow().len();
    let data = v8::Integer::new_from_unsigned(scope, index as u32);
    let function = v8::Function::builder(trampoline)
        .data(data.into())
        .length(def.arity.into())
        .build(scope)
        .ok_or_else(|| internal("could not create host function"))?;
    let name = v8::String::new(scope, def.name).ok_or_else(|| internal("invalid name"))?;
    let global = scope.get_current_context().global(scope);
    if global.set(scope, name.into(), function.into()) != Some(true) {
        return Err(internal("could not install host function"));
    }
    shared.host_fns.borrow_mut().push(def.callback);
    shared.host_names.borrow_mut().insert(def.name, index);
    Ok(())
}

/// Feeds V8's promise-reject events into [`Shared`] for `take_unhandled_rejections`.
#[allow(unsafe_code)]
pub(crate) unsafe extern "C" fn on_promise_reject(message: v8::PromiseRejectMessage) {
    // SAFETY: V8 calls this synchronously in the entered isolate, so the scope cannot outlive it.
    v8::callback_scope!(unsafe scope, &message);
    v8::scope!(let scope, scope);
    let Some(shared) = scope.get_slot::<SharedSlot>().map(|s| Rc::clone(&s.0)) else {
        return;
    };
    shared.track_rejection(scope, message.get_promise(), message.get_event());
}

fn trampoline(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments<'_>,
    mut rv: v8::ReturnValue<'_>,
) {
    let Some(shared) = scope.get_slot::<SharedSlot>().map(|s| Rc::clone(&s.0)) else {
        return throw_message(scope, "host function called outside a Velyr isolate");
    };
    let callback = v8::Local::<v8::Integer>::try_from(args.data())
        .ok()
        .and_then(|index| usize::try_from(index.value()).ok())
        .and_then(|index| shared.host_fns.borrow().get(index).copied());
    let Some(callback) = callback else {
        return throw_message(scope, "unknown host function");
    };

    // Handles created for this call (args, ctx objects) die with it.
    shared.handles.borrow_mut().push_scope();
    let values: Vec<Value> = {
        let mut handles = shared.handles.borrow_mut();
        (0..args.length())
            .map(|i| from_v8(scope, &mut handles, args.get(i)))
            .collect()
    };
    let result = call_host(scope, &shared, callback, &values);
    match result {
        Ok(value) => match to_v8(scope, &shared.handles.borrow(), &value) {
            Ok(value) => rv.set(value),
            Err(err) => throw_message(scope, &err.message),
        },
        Err(err) => throw_host_error(scope, &shared, err),
    }
    shared.handles.borrow_mut().pop_scope();
}

/// Calls the registered host function `name` from engine code (not from JS). `None` when no
/// such function is registered.
pub(crate) fn call_named(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    name: &str,
    args: &[Value],
) -> Option<Result<Value, HostError>> {
    let index = *shared.host_names.borrow().get(name)?;
    let callback = *shared.host_fns.borrow().get(index)?;
    shared.handles.borrow_mut().push_scope();
    let result = call_host(scope, shared, callback, args);
    shared.handles.borrow_mut().pop_scope();
    Some(result)
}

fn call_host(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    callback: HostFn,
    args: &[Value],
) -> Result<Value, HostError> {
    let mut ctx = HostContext {
        scope,
        shared,
        slots: None,
    };
    callback(&mut ctx, args)
}

fn throw_message(scope: &mut v8::PinScope<'_, '_>, message: &str) {
    let message = v8::String::new(scope, message).unwrap_or_else(|| v8::String::empty(scope));
    let error = v8::Exception::error(scope, message);
    scope.throw_exception(error);
}

fn throw_host_error(scope: &mut v8::PinScope<'_, '_>, shared: &Shared, err: HostError) {
    match host_error_to_js(scope, shared, err) {
        Ok(exception) => {
            scope.throw_exception(exception);
        }
        Err(message) => throw_message(scope, &message),
    }
}

/// The JS value a failed host function throws; `Err` carries a message when it cannot be built.
pub(crate) fn host_error_to_js<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    shared: &Shared,
    err: HostError,
) -> Result<v8::Local<'s, v8::Value>, String> {
    Ok(match err {
        HostError::TypeError(message) => {
            let message =
                v8::String::new(scope, &message).unwrap_or_else(|| v8::String::empty(scope));
            v8::Exception::type_error(scope, message)
        }
        HostError::Thrown(value) => {
            to_v8(scope, &shared.handles.borrow(), &value).map_err(|err| err.message)?
        }
        HostError::Velyr(err) => velyr_error_to_js(scope, &err),
    })
}

/// Copies `bytes` into a new `ArrayBuffer` and registers it as a handle.
pub(crate) fn new_array_buffer(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    bytes: &[u8],
) -> Value {
    let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes.to_vec()).make_shared();
    let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
    from_v8(scope, &mut shared.handles.borrow_mut(), buffer.into())
}

/// `Error` whose `code` property is the stable `VELYR_*` string.
pub(crate) fn velyr_error_to_js<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    err: &VelyrError,
) -> v8::Local<'s, v8::Value> {
    let message = v8::String::new(scope, err.message()).unwrap_or_else(|| v8::String::empty(scope));
    let error = v8::Exception::error(scope, message);
    if let (Ok(object), Some(key), Some(code)) = (
        v8::Local::<v8::Object>::try_from(error),
        v8::String::new(scope, "code"),
        v8::String::new(scope, err.code().as_str()),
    ) {
        // Cannot fail on a fresh, extensible Error object.
        let _ = object.set(scope, key.into(), code.into());
    }
    error
}

struct HostContext<'a, 's, 'i> {
    scope: &'a mut v8::PinScope<'s, 'i>,
    shared: &'a Shared,
    /// Held once `slot` is first called, so the returned reference can borrow from `self`.
    slots: Option<RefMut<'a, HashMap<TypeId, Box<dyn Any>>>>,
}

impl CallContext for HostContext<'_, '_, '_> {
    fn create_string(&mut self, value: JsString) -> Value {
        Value::String(value)
    }

    fn create_object(&mut self) -> Value {
        let object = v8::Object::new(self.scope);
        from_v8(
            self.scope,
            &mut self.shared.handles.borrow_mut(),
            object.into(),
        )
    }

    fn create_array(&mut self, items: &[Value]) -> Value {
        let handles = self.shared.handles.borrow();
        let items: Vec<_> = items
            .iter()
            .map(|item| {
                to_v8(self.scope, &handles, item)
                    .unwrap_or_else(|_| v8::undefined(self.scope).into())
            })
            .collect();
        drop(handles);
        let array = v8::Array::new_with_elements(self.scope, &items);
        from_v8(
            self.scope,
            &mut self.shared.handles.borrow_mut(),
            array.into(),
        )
    }

    fn create_array_buffer(&mut self, bytes: &[u8]) -> Value {
        new_array_buffer(self.scope, self.shared, bytes)
    }

    fn read_bytes(&mut self, value: &Value) -> Option<Vec<u8>> {
        let value = to_v8(self.scope, &self.shared.handles.borrow(), value).ok()?;
        let view = if let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(value) {
            view
        } else {
            let buffer = v8::Local::<v8::ArrayBuffer>::try_from(value).ok()?;
            v8::Uint8Array::new(self.scope, buffer, 0, buffer.byte_length())?.into()
        };
        let mut bytes = vec![0; view.byte_length()];
        view.copy_contents(&mut bytes);
        Some(bytes)
    }

    fn get_property(&mut self, object: &Value, key: &str) -> Result<Value, HostError> {
        v8::tc_scope!(let tc, &mut *self.scope);
        let handles = self.shared.handles.borrow();
        let object =
            to_v8(tc, &handles, object).map_err(|err| HostError::TypeError(err.message))?;
        drop(handles);
        let object = v8::Local::<v8::Object>::try_from(object)
            .map_err(|_| HostError::TypeError("not an object".into()))?;
        let key =
            v8::String::new(tc, key).ok_or_else(|| HostError::TypeError("key too long".into()))?;
        match object.get(tc, key.into()) {
            Some(value) => Ok(from_v8(tc, &mut self.shared.handles.borrow_mut(), value)),
            None => Err(caught(tc, self.shared)),
        }
    }

    fn set_property(&mut self, object: &Value, key: &str, value: Value) -> Result<(), HostError> {
        v8::tc_scope!(let tc, &mut *self.scope);
        let handles = self.shared.handles.borrow();
        let converted = to_v8(tc, &handles, object)
            .and_then(|o| to_v8(tc, &handles, &value).map(|v| (o, v)))
            .map_err(|err| HostError::TypeError(err.message))?;
        drop(handles);
        let (object, value) = converted;
        let object = v8::Local::<v8::Object>::try_from(object)
            .map_err(|_| HostError::TypeError("not an object".into()))?;
        let key =
            v8::String::new(tc, key).ok_or_else(|| HostError::TypeError("key too long".into()))?;
        match object.set(tc, key.into(), value) {
            Some(_) => Ok(()),
            None => Err(caught(tc, self.shared)),
        }
    }

    fn create_promise(&mut self) -> (Value, ResolverId) {
        self.shared.new_promise(self.scope)
    }

    fn resolve(&mut self, resolver: ResolverId, value: Value) {
        self.shared.settle(self.scope, resolver, &value, true);
    }

    fn reject(&mut self, resolver: ResolverId, reason: Value) {
        self.shared.settle(self.scope, resolver, &reason, false);
    }

    fn throw_type_error(&mut self, message: &str) -> HostError {
        HostError::TypeError(message.to_owned())
    }

    fn serialize(
        &mut self,
        value: &Value,
        transfer: &[Value],
    ) -> Result<SerializedValue, HostError> {
        v8::tc_scope!(let tc, &mut *self.scope);
        serialize::serialize(tc, self.shared, value, transfer)
            .ok_or_else(|| caught(tc, self.shared))
    }

    fn deserialize(&mut self, serialized: &SerializedValue) -> Result<Value, HostError> {
        v8::tc_scope!(let tc, &mut *self.scope);
        serialize::deserialize(tc, self.shared, serialized).ok_or_else(|| caught(tc, self.shared))
    }

    fn compile_module(&mut self, source: &Source) -> Result<Handle, HostError> {
        modules::compile(self.scope, self.shared, source, None).map_err(exception_to_host)
    }

    fn instantiate_module(
        &mut self,
        module: Handle,
        hook: &mut dyn ModuleResolveHook,
    ) -> Result<(), HostError> {
        self.slots = None;
        modules::instantiate(self.scope, self.shared, module, hook).map_err(exception_to_host)
    }

    fn evaluate_module(&mut self, module: Handle) -> Result<Value, HostError> {
        // Module code can call host functions that read slots; release our borrow first.
        self.slots = None;
        let promise =
            modules::evaluate(self.scope, self.shared, module).map_err(exception_to_host)?;
        // The caller reads the outcome itself, so a rejection must not also surface as unhandled.
        self.shared.mark_handled(self.scope, &promise);
        Ok(promise)
    }

    fn module_namespace(&mut self, module: Handle) -> Result<Value, HostError> {
        modules::namespace(self.scope, self.shared, module).map_err(exception_to_host)
    }

    fn promise_state(&mut self, promise: &Value) -> Option<PromiseState> {
        promise_state(self.scope, self.shared, promise)
    }

    fn slot(&mut self, type_id: TypeId) -> Option<&mut dyn Any> {
        let shared = self.shared;
        let slots = self.slots.get_or_insert_with(|| shared.slots.borrow_mut());
        Some(slots.get_mut(&type_id)?.as_mut())
    }
}

/// Converts the exception a failed JS operation left in `tc`, so the host fn can propagate it.
fn caught(tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>, shared: &Shared) -> HostError {
    match tc.exception() {
        Some(exception) => {
            HostError::Thrown(from_v8(tc, &mut shared.handles.borrow_mut(), exception))
        }
        None => HostError::TypeError("operation failed".into()),
    }
}

fn exception_to_host(exception: JsException) -> HostError {
    match exception.value {
        Some(value) => HostError::Thrown(value),
        None => HostError::Velyr(VelyrError::invalid_data(exception.message)),
    }
}

pub(crate) fn promise_state(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    promise: &Value,
) -> Option<PromiseState> {
    let value = to_v8(scope, &shared.handles.borrow(), promise).ok()?;
    let promise = v8::Local::<v8::Promise>::try_from(value).ok()?;
    let state = promise.state();
    if state == v8::PromiseState::Pending {
        return Some(PromiseState::Pending);
    }
    let result = promise.result(scope);
    let value = from_v8(scope, &mut shared.handles.borrow_mut(), result);
    Some(match state {
        v8::PromiseState::Fulfilled => PromiseState::Fulfilled(value),
        _ => PromiseState::Rejected(value),
    })
}

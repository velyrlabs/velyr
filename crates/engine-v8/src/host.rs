//! Host functions: the V8 trampoline and the concrete [`CallContext`].

use std::any::{Any, TypeId};
use std::cell::RefMut;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use velyr_engine_api::{
    CallContext, Handle, HostError, HostFn, HostFunctionDef, JsException, JsString,
    ModuleResolveHook, NativeError, NativeScope, NativeScopeFn, NativeTeardown, NativeValue,
    PromiseState, ResolverId, SerializedValue, Source, TickFlags, V8_ENGINE, Value,
};
use velyr_runtime_errors::VelyrError;

use crate::code_cache;
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

pub(crate) fn trampoline(
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

/// Isolate-slot list of the [`NativeTeardown`]s registered through `on_native_teardown`.
struct NativeTeardowns(Vec<NativeTeardown>);

/// Lends `scope` to FFI code as a [`NativeScope`] tagged [`V8_ENGINE`].
fn native_scope<'a>(scope: &'a mut v8::PinScope<'_, '_>) -> NativeScope<'a> {
    NativeScope::new(V8_ENGINE, NonNull::from(scope).cast())
}

/// Runs the [`NativeTeardown`]s registered on this isolate. Call once, while disposing it.
pub(crate) fn run_native_teardown(scope: &mut v8::PinScope<'_, '_>, shared: &Shared) {
    let hooks = shared
        .slots
        .borrow_mut()
        .remove(&TypeId::of::<NativeTeardowns>());
    let Some(Ok(hooks)) = hooks.map(|hooks| hooks.downcast::<NativeTeardowns>()) else {
        return;
    };
    for teardown in hooks.0 {
        teardown(&mut native_scope(scope));
    }
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

    fn with_bytes(&mut self, value: &Value, f: &mut dyn FnMut(&[u8])) -> bool {
        self.with_bytes_mut(value, &mut |bytes| f(bytes))
    }

    #[allow(unsafe_code)]
    fn with_bytes_mut(&mut self, value: &Value, f: &mut dyn FnMut(&mut [u8])) -> bool {
        let Ok(value) = to_v8(self.scope, &self.shared.handles.borrow(), value) else {
            return false;
        };
        let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(value) else {
            return false;
        };
        // Materializes an on-heap typed array so `data()` points at stable off-heap memory.
        // A SharedArrayBuffer can change under us from another thread, so it keeps the copy path.
        match view.buffer(self.scope) {
            Some(buffer) if !v8::Local::<v8::Value>::from(buffer).is_shared_array_buffer() => {}
            _ => return false,
        }
        let len = view.byte_length();
        let ptr = view.data().cast::<u8>();
        let window: &mut [u8] = if len == 0 || ptr.is_null() {
            &mut []
        } else {
            // SAFETY: `ptr..ptr + len` is the view's live backing-store window; `f` runs no JS, so
            // nothing can detach or resize the buffer, and this is the only reference to it.
            unsafe { std::slice::from_raw_parts_mut(ptr, len) }
        };
        f(window);
        true
    }

    #[allow(unsafe_code)]
    fn write_string_utf8(&mut self, text: &JsString, out: &mut Vec<u8>) {
        if let Some(bytes) = text.as_latin1() {
            write_latin1_as_utf8(bytes, out);
            return;
        }
        let units = text.as_utf16();
        // Lone surrogates are rare: the lossy path keeps their U+FFFD.
        if !v8::simdutf::validate_utf16le(units) {
            out.extend_from_slice(text.to_string_lossy().as_bytes());
            return;
        }
        let start = out.len();
        out.resize(start + v8::simdutf::utf8_length_from_utf16le(units), 0);
        // SAFETY: `units` was validated above and `out[start..]` holds exactly the UTF-8 length.
        let written =
            unsafe { v8::simdutf::convert_valid_utf16le_to_utf8(units, &mut out[start..]) };
        debug_assert_eq!(start + written, out.len());
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
        // Serialization reads `Shared::slots`; release the borrow `slot` may hold.
        self.slots = None;
        v8::tc_scope!(let tc, &mut *self.scope);
        serialize::serialize(tc, self.shared, value, transfer)
            .ok_or_else(|| caught(tc, self.shared))
    }

    fn deserialize(&mut self, serialized: &SerializedValue) -> Result<Value, HostError> {
        self.slots = None;
        v8::tc_scope!(let tc, &mut *self.scope);
        serialize::deserialize(tc, self.shared, serialized).ok_or_else(|| caught(tc, self.shared))
    }

    fn eval_script(&mut self, source: &Source) -> Result<Value, HostError> {
        crate::script(self.scope, self.shared, source, true).map_err(exception_to_host)
    }

    fn enqueue_microtask(&mut self, func: &Value) -> Result<bool, HostError> {
        let value = to_v8(self.scope, &self.shared.handles.borrow(), func)
            .map_err(|err| HostError::TypeError(err.message))?;
        let func = v8::Local::<v8::Function>::try_from(value)
            .map_err(|_| HostError::TypeError("microtask must be a function".into()))?;
        self.scope.enqueue_microtask(func);
        self.shared.microtasks_dirty.set(true);
        Ok(true)
    }

    fn run_microtask_checkpoint(&mut self) {
        self.scope.perform_microtask_checkpoint();
    }

    fn request_gc(&mut self) -> Result<(), HostError> {
        self.scope.low_memory_notification();
        Ok(())
    }

    fn compile_module(&mut self, source: &Source) -> Result<Handle, HostError> {
        modules::compile(self.scope, self.shared, source, None).map_err(exception_to_host)
    }

    fn compile_function(
        &mut self,
        source: &Source,
        params: &[&str],
        cache: Option<&[u8]>,
    ) -> Result<Value, HostError> {
        code_cache::compile_function(self.scope, self.shared, source, params, cache)
            .map_err(exception_to_host)
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

    fn set_continuation_context(&mut self, value: Value) {
        crate::continuation::set(self.scope, self.shared, &value);
    }

    fn get_continuation_context(&mut self) -> Value {
        crate::continuation::get(self.scope, self.shared)
    }

    #[allow(unsafe_code)]
    fn with_native_scope(&mut self, f: &mut NativeScopeFn<'_>) -> Result<Value, HostError> {
        // Native code can run JS whose host functions read slots; release our borrow first.
        self.slots = None;
        let result = f(&mut native_scope(self.scope));
        let mut convert = |raw: NativeValue| {
            // SAFETY: per the `V8_ENGINE` contract the callback returned the slot pointer of a
            // `Local` created in the scope it was lent, which is still open; same layout.
            let value =
                unsafe { std::mem::transmute::<NativeValue, v8::Local<'_, v8::Value>>(raw) };
            from_v8(self.scope, &mut self.shared.handles.borrow_mut(), value)
        };
        match result {
            Ok(raw) => Ok(convert(raw)),
            Err(NativeError::Thrown(raw)) => Err(HostError::Thrown(convert(raw))),
            Err(NativeError::Host(err)) => Err(err),
        }
    }

    fn on_native_teardown(&mut self, teardown: NativeTeardown) -> Result<(), HostError> {
        let shared = self.shared;
        let slots = self.slots.get_or_insert_with(|| shared.slots.borrow_mut());
        let hooks = slots
            .entry(TypeId::of::<NativeTeardowns>())
            .or_insert_with(|| Box::new(NativeTeardowns(Vec::new())));
        if let Some(NativeTeardowns(hooks)) = hooks.downcast_mut::<NativeTeardowns>()
            && !hooks
                .iter()
                .any(|hook| std::ptr::fn_addr_eq(*hook, teardown))
        {
            hooks.push(teardown);
        }
        Ok(())
    }

    #[allow(unsafe_code)]
    fn bind_tick_flags(&mut self, flags: &TickFlags) -> Option<Value> {
        unsafe extern "C" fn release(_: *mut c_void, _: usize, owner: *mut c_void) {
            // SAFETY: `owner` is the `Box<TickFlags>` leaked below; V8 calls this once.
            drop(unsafe { Box::from_raw(owner.cast::<TickFlags>()) });
        }
        let owner = Box::into_raw(Box::new(flags.clone()));
        // SAFETY: the bytes live in the Arc that `owner` keeps alive until V8 frees the store, and
        // JS and Rust only touch them on the isolate thread.
        let store = unsafe {
            v8::ArrayBuffer::new_backing_store_from_ptr(
                flags.as_ptr().cast(),
                2,
                release,
                owner.cast(),
            )
        }
        .make_shared();
        let buffer = v8::ArrayBuffer::with_backing_store(self.scope, &store);
        let view = v8::Uint8Array::new(self.scope, buffer, 0, 2)?;
        Some(from_v8(
            self.scope,
            &mut self.shared.handles.borrow_mut(),
            view.into(),
        ))
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

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use velyr_engine_api::{Engine, Isolate, IsolateOptions};

    use super::*;
    use crate::V8Engine;

    fn isolate_with(name: &'static str, callback: HostFn) -> crate::V8Isolate {
        let mut isolate = V8Engine::new()
            .create_isolate(IsolateOptions::default())
            .unwrap();
        isolate
            .register_host_function(HostFunctionDef {
                name,
                arity: 1,
                callback,
            })
            .unwrap();
        isolate
    }

    fn eval(isolate: &mut crate::V8Isolate, code: &str) -> Value {
        let source = Source {
            name: "test.js".into(),
            text: code.into(),
        };
        isolate.eval_script(&source).unwrap()
    }

    /// Returns `"native"`, or throws `7` when called with `true`, from inside a native scope.
    fn op_native(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
        let throw = matches!(args.first(), Some(Value::Bool(true)));
        ctx.with_native_scope(&mut |lent| {
            assert_eq!(lent.engine(), V8_ENGINE);
            // SAFETY: a `V8_ENGINE` scope points at a live `PinScope` for the callback.
            let scope = unsafe { lent.as_ptr().cast::<v8::PinScope<'_, '_>>().as_mut() };
            let value: v8::Local<'_, v8::Value> = if throw {
                v8::Integer::new(scope, 7).into()
            } else {
                v8::String::new(scope, "native").unwrap().into()
            };
            // SAFETY: `Local` is a single non-null slot pointer.
            let raw =
                unsafe { std::mem::transmute::<v8::Local<'_, v8::Value>, NativeValue>(value) };
            if throw {
                Err(NativeError::Thrown(raw))
            } else {
                Ok(raw)
            }
        })
    }

    #[test]
    fn native_scope_values_and_throws_reach_js() {
        let mut isolate = isolate_with("op_native", op_native);
        let result = eval(
            &mut isolate,
            "let thrown; try { op_native(true); } catch (e) { thrown = e; } op_native() + ':' + thrown",
        );
        assert_eq!(result, Value::String(JsString::from("native:7")));
    }

    static TEARDOWNS: AtomicUsize = AtomicUsize::new(0);

    fn count_teardown(scope: &mut NativeScope<'_>) {
        assert_eq!(scope.engine(), V8_ENGINE);
        TEARDOWNS.fetch_add(1, Ordering::SeqCst);
    }

    fn op_register(ctx: &mut dyn CallContext, _args: &[Value]) -> Result<Value, HostError> {
        ctx.on_native_teardown(count_teardown)?;
        Ok(Value::Undefined)
    }

    #[test]
    fn native_teardown_runs_once_when_the_isolate_is_dropped() {
        let mut isolate = isolate_with("op_register", op_register);
        eval(&mut isolate, "op_register(); op_register();");
        assert_eq!(TEARDOWNS.load(Ordering::SeqCst), 0);
        drop(isolate);
        assert_eq!(TEARDOWNS.load(Ordering::SeqCst), 1);
    }
}

fn write_latin1_as_utf8(bytes: &[u8], out: &mut Vec<u8>) {
    if bytes.is_ascii() {
        out.extend_from_slice(bytes);
        return;
    }
    let mut utf8 = [0u8; 2];
    for &byte in bytes {
        out.extend_from_slice(char::from(byte).encode_utf8(&mut utf8).as_bytes());
    }
}

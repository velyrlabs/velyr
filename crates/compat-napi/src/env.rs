//! `napi_env`: per-module state, exceptions, native callbacks and GC finalizers.
//!
//! An env is reached re-entrantly (a native callback calls JS that calls another native
//! callback), so its state uses `Cell`/`RefCell` behind `&Env` and no borrow is held across
//! a call into JS or V8 allocation.

use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;

use crate::ffi::local;
use crate::scopes::OpenScope;
use crate::types::{
    ExtendedErrorInfo, Status, napi_callback, napi_callback_info, napi_env, napi_finalize,
};

/// The `napi_finalize` an addon attached to some data.
#[derive(Clone, Copy)]
pub(crate) struct Finalizer {
    pub(crate) callback: napi_finalize,
    pub(crate) data: *mut c_void,
    pub(crate) hint: *mut c_void,
}

impl Finalizer {
    pub(crate) fn run(self, env: &Env) {
        if let Some(callback) = self.callback {
            // SAFETY: an addon-supplied finalizer called with the arguments it registered.
            unsafe { callback(env.raw(), self.data, self.hint) };
        }
    }
}

/// Native data attached to a JS object by `napi_wrap`; owned by the object's [`Tracked`] entry.
pub(crate) struct Wrap {
    pub(crate) data: *mut c_void,
    pub(crate) id: u64,
}

/// A finalizer waiting for its object to be collected.
struct Tracked {
    // Dropping the weak handle cancels its GC callback.
    _weak: v8::Weak<v8::Value>,
    finalizer: Finalizer,
    _wrap: Option<Box<Wrap>>,
}

/// What a native function was created with; the function's data is an `External` over it.
pub(crate) struct CallbackData {
    env: *const Env,
    callback: napi_callback,
    data: *mut c_void,
}

/// What `napi_callback_info` points at for the duration of one native call.
pub(crate) struct CallbackInfo {
    /// A `v8::FunctionCallbackArguments` living on the trampoline's stack.
    pub(crate) args: *const c_void,
    pub(crate) data: *mut c_void,
}

/// The envs of one isolate, kept in an isolate slot until [`crate::shutdown`].
#[allow(
    clippy::vec_box,
    reason = "addons hold `napi_env` pointers, so each env needs a stable address"
)]
pub(crate) struct EnvSlot(pub(crate) Vec<Box<Env>>);

pub struct Env {
    /// Persistent handle to the module's context, from `Global::into_raw`.
    context: NonNull<v8::Context>,
    pub(crate) module_api_version: i32,
    last_status: Cell<Status>,
    error_info: UnsafeCell<ExtendedErrorInfo>,
    pending_exception: RefCell<Option<v8::Global<v8::Value>>>,
    pub(crate) scopes: RefCell<Vec<OpenScope>>,
    #[allow(
        clippy::vec_box,
        reason = "functions point at their record, so it needs a stable address"
    )]
    callbacks: RefCell<Vec<Box<CallbackData>>>,
    tracked: RefCell<HashMap<u64, Tracked>>,
    /// Ids whose objects the GC collected; their finalizers run at the next safe point.
    collected: RefCell<Vec<u64>>,
    next_id: Cell<u64>,
    pub(crate) instance_data: Cell<Option<Finalizer>>,
}

impl Env {
    /// Creates the env for the current context and hands ownership to the isolate.
    pub(crate) fn create(
        scope: &mut v8::PinScope<'_, '_>,
        module_api_version: i32,
    ) -> &'static Env {
        let context = scope.get_current_context();
        let env = Box::new(Env {
            context: v8::Global::new(scope, context).into_raw(),
            module_api_version,
            last_status: Cell::new(Status::Ok),
            error_info: UnsafeCell::new(ExtendedErrorInfo {
                error_message: std::ptr::null(),
                engine_reserved: std::ptr::null_mut(),
                engine_error_code: 0,
                error_code: Status::Ok,
            }),
            pending_exception: RefCell::new(None),
            scopes: RefCell::new(Vec::new()),
            callbacks: RefCell::new(Vec::new()),
            tracked: RefCell::new(HashMap::new()),
            collected: RefCell::new(Vec::new()),
            next_id: Cell::new(0),
            instance_data: Cell::new(None),
        });
        let ptr: *const Env = &*env;
        match scope.get_slot_mut::<EnvSlot>() {
            Some(slot) => slot.0.push(env),
            None => {
                scope.set_slot(EnvSlot(vec![env]));
            }
        }
        // SAFETY: the box is owned by the isolate slot and freed only by `shutdown`.
        unsafe { &*ptr }
    }

    pub(crate) fn from_raw(env: napi_env) -> Option<&'static Env> {
        // SAFETY: a non-null `napi_env` is one this crate handed out; it stays valid until
        // `shutdown` (addons must not use an env after teardown, as in Node).
        unsafe { env.as_ref() }
    }

    pub(crate) fn raw(&self) -> napi_env {
        self
    }

    pub(crate) fn context<'s>(&self) -> v8::Local<'s, v8::Context> {
        // SAFETY: `context` is a live persistent handle slot, laid out like a `Local`.
        unsafe {
            std::mem::transmute::<NonNull<v8::Context>, v8::Local<'s, v8::Context>>(self.context)
        }
    }

    pub(crate) fn record(&self, result: Result<(), Status>) -> Status {
        let status = result.err().unwrap_or(Status::Ok);
        self.last_status.set(status);
        status
    }

    /// `napi_get_last_error_info`: valid until the next Node-API call on this env.
    pub(crate) fn error_info(&self) -> *const ExtendedErrorInfo {
        let status = self.last_status.get();
        let info = self.error_info.get();
        // SAFETY: only this thread touches the env and no reference into it is live here.
        unsafe {
            (*info).error_code = status;
            (*info).error_message = status.message();
        }
        info
    }

    pub(crate) fn has_pending_exception(&self) -> bool {
        self.pending_exception.borrow().is_some()
    }

    pub(crate) fn set_pending_exception(
        &self,
        isolate: &v8::Isolate,
        value: v8::Local<'_, v8::Value>,
    ) {
        *self.pending_exception.borrow_mut() = Some(v8::Global::new(isolate, value));
    }

    pub(crate) fn take_pending_exception<'s>(
        &self,
        scope: &v8::PinScope<'s, '_, ()>,
    ) -> Option<v8::Local<'s, v8::Value>> {
        let exception = self.pending_exception.borrow_mut().take()?;
        Some(v8::Local::new(scope, exception))
    }

    /// Runs `f`, which may call into JS. An exception it throws becomes the env's pending
    /// exception and `PendingException`; nothing runs while one is already pending.
    pub(crate) fn run_js<T>(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        f: impl FnOnce(&mut v8::PinScope<'_, '_>) -> Option<T>,
    ) -> Result<T, Status> {
        if self.has_pending_exception() {
            return Err(Status::PendingException);
        }
        v8::tc_scope!(let tc, scope);
        if let Some(value) = f(tc) {
            return Ok(value);
        }
        let Some(exception) = tc.exception() else {
            return Err(Status::GenericFailure);
        };
        self.set_pending_exception(tc, exception);
        Err(Status::PendingException)
    }

    fn callback_data(
        &self,
        callback: napi_callback,
        data: *mut c_void,
    ) -> Result<*mut c_void, Status> {
        if callback.is_none() {
            return Err(Status::InvalidArg);
        }
        let mut record = Box::new(CallbackData {
            env: self,
            callback,
            data,
        });
        let ptr: *mut CallbackData = &mut *record;
        self.callbacks.borrow_mut().push(record);
        Ok(ptr.cast())
    }

    pub(crate) fn function_template<'s>(
        &self,
        scope: &v8::PinScope<'s, '_, ()>,
        callback: napi_callback,
        data: *mut c_void,
    ) -> Result<v8::Local<'s, v8::FunctionTemplate>, Status> {
        let external = v8::External::new(scope, self.callback_data(callback, data)?);
        Ok(v8::FunctionTemplate::builder(trampoline)
            .data(external.into())
            .build(scope))
    }

    pub(crate) fn function<'s>(
        &self,
        scope: &v8::PinScope<'s, '_>,
        name: Option<v8::Local<'s, v8::String>>,
        callback: napi_callback,
        data: *mut c_void,
    ) -> Result<v8::Local<'s, v8::Function>, Status> {
        let external = v8::External::new(scope, self.callback_data(callback, data)?);
        let function = v8::Function::builder(trampoline)
            .data(external.into())
            .build(scope)
            .ok_or(Status::GenericFailure)?;
        if let Some(name) = name {
            function.set_name(name);
        }
        Ok(function)
    }

    /// Runs `finalizer` once `value` is garbage collected (or at teardown). Returns its id.
    pub(crate) fn track(
        &self,
        isolate: &mut v8::Isolate,
        value: v8::Local<'_, v8::Value>,
        finalizer: Finalizer,
        wrap: Option<Box<Wrap>>,
    ) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let env: *const Env = self;
        let on_collect = Box::new(move |_: &mut v8::Isolate| {
            // SAFETY: the weak handle owning this callback lives in `env.tracked`, so the env is
            // alive whenever V8 can still call it.
            unsafe { &*env }.collected.borrow_mut().push(id);
        });
        let weak = v8::Weak::with_finalizer(isolate, value, on_collect);
        let wrap = wrap.map(|mut wrap| {
            wrap.id = id;
            wrap
        });
        let tracked = Tracked {
            _weak: weak,
            finalizer,
            _wrap: wrap,
        };
        self.tracked.borrow_mut().insert(id, tracked);
        id
    }

    /// Forgets a tracked finalizer without running it (`napi_remove_wrap`).
    pub(crate) fn untrack(&self, id: u64) {
        let removed = self.tracked.borrow_mut().remove(&id);
        drop(removed);
    }

    /// Closes handle scopes opened after the first `depth`, innermost first (V8 needs LIFO).
    pub(crate) fn close_scopes_above(&self, depth: usize) {
        let mut scopes = self.scopes.borrow_mut();
        while scopes.len() > depth {
            scopes.pop();
        }
    }

    /// Runs the finalizers of collected objects. V8 only records them during GC; they run here,
    /// where calling back into the addon (and into Node-API) is safe.
    pub(crate) fn drain_finalizers(&self) {
        loop {
            let ids = std::mem::take(&mut *self.collected.borrow_mut());
            if ids.is_empty() {
                return;
            }
            for id in ids {
                let tracked = self.tracked.borrow_mut().remove(&id);
                if let Some(tracked) = tracked {
                    tracked.finalizer.run(self);
                }
            }
        }
    }

    /// Env teardown: pending and outstanding finalizers, then the instance data finalizer.
    pub(crate) fn teardown(self: Box<Self>, isolate: &mut v8::Isolate) {
        self.drain_finalizers();
        let outstanding: Vec<Tracked> = self.tracked.borrow_mut().drain().map(|(_, t)| t).collect();
        for tracked in outstanding {
            tracked.finalizer.run(&self);
        }
        if let Some(finalizer) = self.instance_data.take() {
            finalizer.run(&self);
        }
        self.close_scopes_above(0);
        // SAFETY: `context` came from `Global::into_raw` in `create` and is released only here.
        drop(unsafe { v8::Global::from_raw(isolate, self.context) });
    }
}

/// Entry point of every function created through Node-API.
fn trampoline(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments<'_>,
    mut rv: v8::ReturnValue<'_>,
) {
    let Ok(external) = v8::Local::<v8::External>::try_from(args.data()) else {
        return;
    };
    // SAFETY: functions created by this crate carry an External over a `CallbackData` that
    // lives in its env's `callbacks` until teardown.
    let record = unsafe { &*external.value().cast::<CallbackData>() };
    // SAFETY: the env outlives every function it created (see `Env::from_raw`).
    let env = unsafe { &*record.env };
    let Some(callback) = record.callback else {
        return;
    };
    env.drain_finalizers();
    let depth = env.scopes.borrow().len();
    let info = CallbackInfo {
        args: (&raw const args).cast(),
        data: record.data,
    };
    // SAFETY: the addon callback receives its env and an info valid for this call.
    let result = unsafe {
        callback(
            env.raw(),
            (&raw const info).cast::<c_void>() as napi_callback_info,
        )
    };
    if let Some(exception) = env.take_pending_exception(scope) {
        scope.throw_exception(exception);
    } else if let Ok(value) = local(result) {
        rv.set(value);
    }
    // Scopes the callback left open would otherwise outlive it.
    env.close_scopes_above(depth);
}

pub(crate) fn callback_info<'a>(info: napi_callback_info) -> Result<&'a CallbackInfo, Status> {
    // SAFETY: a non-null `napi_callback_info` points at the trampoline's `CallbackInfo`, which
    // lives until the callback returns (N-API contract).
    unsafe { info.cast::<CallbackInfo>().as_ref() }.ok_or(Status::InvalidArg)
}

pub(crate) fn arguments<'a>(info: &CallbackInfo) -> &'a v8::FunctionCallbackArguments<'a> {
    // SAFETY: `args` points at the arguments of the native call in progress.
    unsafe { &*info.args.cast::<v8::FunctionCallbackArguments<'a>>() }
}

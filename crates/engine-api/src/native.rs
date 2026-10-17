//! Raw engine access for FFI layers (Node-API) that hand engine handles to C code.
//!
//! ADR-009 keeps engine types out of this crate, so the scope and values cross as opaque
//! pointers tagged with the engine that lent them. Only an engine adapter builds a
//! [`NativeScope`]; a consumer checks [`NativeScope::engine`] before reading the pointer.

use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::HostError;

/// Tag of scopes lent by `velyr-engine-v8`: the pointer is a `*mut v8::PinScope<'_, '_>` with the
/// isolate's context entered, and a [`NativeValue`] is the slot pointer of a `v8::Local<v8::Value>`.
pub const V8_ENGINE: &str = "v8";

/// The engine's own scope, lent for one callback; the borrow keeps it from escaping.
#[derive(Debug)]
pub struct NativeScope<'a> {
    engine: &'static str,
    raw: NonNull<c_void>,
    _lent: PhantomData<&'a mut ()>,
}

impl NativeScope<'_> {
    /// For engine adapters: `raw` must stay valid while this value lives and have the layout
    /// documented for `engine`.
    pub fn new(engine: &'static str, raw: NonNull<c_void>) -> Self {
        Self {
            engine,
            raw,
            _lent: PhantomData,
        }
    }

    pub fn engine(&self) -> &'static str {
        self.engine
    }

    pub fn as_ptr(&mut self) -> NonNull<c_void> {
        self.raw
    }
}

/// A raw engine value created in the [`NativeScope`] a callback received.
pub type NativeValue = NonNull<c_void>;

/// Why a [`NativeScopeFn`] failed.
#[derive(Debug)]
pub enum NativeError {
    /// Throw this engine value.
    Thrown(NativeValue),
    Host(HostError),
}

/// Code that runs inside a [`NativeScope`]: see `CallContext::with_native_scope`.
pub type NativeScopeFn<'f> =
    dyn FnMut(&mut NativeScope<'_>) -> Result<NativeValue, NativeError> + 'f;

/// Runs in a [`NativeScope`] while the isolate is being disposed, before its handles are freed.
pub type NativeTeardown = fn(&mut NativeScope<'_>);

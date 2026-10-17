//! [`load`] and [`shutdown`] for the runtime, which reaches V8 only through an engine-api
//! [`NativeScope`] (ADR-009 keeps `v8` types out of the runtime crates).

use std::path::Path;

use velyr_engine_api::{HostError, NativeError, NativeScope, NativeValue, V8_ENGINE};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_permissions::CapabilitySet;

use crate::ffi::to_napi;
use crate::loader::{LoadError, load, shutdown};

/// [`load`] inside `scope`: the exports, or the exception the module init threw.
pub fn load_native(
    scope: &mut NativeScope<'_>,
    path: &Path,
    permissions: &CapabilitySet,
) -> Result<NativeValue, NativeError> {
    let scope = pin_scope(scope).map_err(|err| NativeError::Host(HostError::Velyr(err)))?;
    match load(scope, path, permissions) {
        Ok(exports) => Ok(raw(exports)),
        Err(LoadError::Exception(exception)) => Err(NativeError::Thrown(raw(exception))),
        Err(LoadError::Velyr(err)) => Err(NativeError::Host(HostError::Velyr(err))),
    }
}

/// [`shutdown`] inside `scope`; a `NativeTeardown` for the isolate's disposal.
pub fn shutdown_native(scope: &mut NativeScope<'_>) {
    if let Ok(scope) = pin_scope(scope) {
        shutdown(scope);
    }
}

fn pin_scope<'a>(
    scope: &'a mut NativeScope<'_>,
) -> Result<&'a mut v8::PinScope<'a, 'a>, VelyrError> {
    if scope.engine() != V8_ENGINE {
        return Err(VelyrError::unsupported("native addons need the V8 engine"));
    }
    // SAFETY: a `V8_ENGINE` scope is lent by velyr-engine-v8 and points at a live `PinScope` with
    // the context entered, exclusively ours for the borrow of `scope`.
    Ok(unsafe { scope.as_ptr().cast::<v8::PinScope<'a, 'a>>().as_mut() })
}

fn raw(value: v8::Local<'_, v8::Value>) -> NativeValue {
    // SAFETY: a `Local` is a non-null slot pointer (`to_napi` keeps the bits).
    unsafe { NativeValue::new_unchecked(to_napi(value)) }
}

//! Pointer plumbing shared by the exported functions. Every raw pointer an addon passes in is
//! read through these helpers, so the null checks and the unsafe reads live in one place.
//!
//! The N-API contract (`js_native_api.h`) is that non-null pointers are valid for the call:
//! out-parameters point at writable storage, strings at `length` bytes (or a NUL-terminated
//! string for `NAPI_AUTO_LENGTH`) and `napi_value`s at live handle slots.

use std::ffi::{CStr, c_char};
use std::ptr::NonNull;

use crate::types::{NAPI_AUTO_LENGTH, Status, napi_value};

/// Defines an exported Node-API function. `$body` sees `$env: &Env`, returns
/// `Result<(), Status>`, and the status becomes the env's last error.
macro_rules! napi_fn {
    ($(#[$meta:meta])* fn $name:ident($env:ident $(, $arg:ident: $ty:ty)* $(,)?) $body:block) => {
        $(#[$meta])*
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(
            $env: $crate::types::napi_env $(, $arg: $ty)*
        ) -> $crate::types::Status {
            let Some($env) = $crate::env::Env::from_raw($env) else {
                return $crate::types::Status::InvalidArg;
            };
            // The closure gives `$body` its own `?` scope.
            #[allow(clippy::redundant_closure_call)]
            let result: Result<(), $crate::types::Status> = (|| $body)();
            $env.record(result)
        }
    };
}
pub(crate) use napi_fn;

/// Binds `$scope` to a `CallbackScope` over the env's context. It opens no handle scope, so
/// handles land in the innermost V8 handle scope, as `napi_value`s must.
macro_rules! napi_scope {
    ($env:ident, $scope:ident) => {
        let context = $env.context();
        // SAFETY: Node-API calls run on the isolate thread inside an open handle scope (a
        // callback, the module init or one the addon opened), and the context is kept alive by
        // the env's global handle.
        v8::callback_scope!(unsafe $scope, context);
    };
}
pub(crate) use napi_scope;

/// The `napi_value` for a handle: the address of its slot.
pub(crate) fn to_napi(local: v8::Local<'_, v8::Value>) -> napi_value {
    // SAFETY: `Local` is `#[repr(C)]` around a single `NonNull` slot pointer.
    unsafe { std::mem::transmute::<v8::Local<'_, v8::Value>, napi_value>(local) }
}

/// The handle behind a `napi_value`; `InvalidArg` for null.
pub(crate) fn local<'s>(value: napi_value) -> Result<v8::Local<'s, v8::Value>, Status> {
    let slot = NonNull::new(value).ok_or(Status::InvalidArg)?;
    // SAFETY: non-null `napi_value`s come from `to_napi` and point at a slot of a live handle
    // scope (N-API contract); `Local` has the same layout as the pointer.
    Ok(unsafe { std::mem::transmute::<NonNull<std::ffi::c_void>, v8::Local<'s, v8::Value>>(slot) })
}

/// Like [`local`], narrowed to `T`, failing with `status` on a type mismatch.
pub(crate) fn local_as<'s, T>(value: napi_value, status: Status) -> Result<v8::Local<'s, T>, Status>
where
    v8::Local<'s, T>: TryFrom<v8::Local<'s, v8::Value>>,
{
    v8::Local::<T>::try_from(local(value)?).map_err(|_| status)
}

/// Writes an out-parameter; `InvalidArg` for null.
pub(crate) fn write<T>(out: *mut T, value: T) -> Result<(), Status> {
    if out.is_null() {
        return Err(Status::InvalidArg);
    }
    // SAFETY: non-null out-parameters point at writable storage for a `T` (N-API contract).
    unsafe { out.write(value) };
    Ok(())
}

/// Writes an optional out-parameter; null means the caller does not want the value.
pub(crate) fn write_opt<T>(out: *mut T, value: T) {
    let _ = write(out, value);
}

/// Bytes of a string argument given as pointer and length (or `NAPI_AUTO_LENGTH`).
pub(crate) fn c_bytes<'a>(ptr: *const c_char, length: usize) -> Result<&'a [u8], Status> {
    if length == NAPI_AUTO_LENGTH {
        if ptr.is_null() {
            return Err(Status::InvalidArg);
        }
        // SAFETY: with NAPI_AUTO_LENGTH the caller passes a NUL-terminated string.
        return Ok(unsafe { CStr::from_ptr(ptr) }.to_bytes());
    }
    slice(ptr.cast::<u8>(), length).and_then(|bytes| {
        if bytes.len() > i32::MAX as usize {
            Err(Status::InvalidArg)
        } else {
            Ok(bytes)
        }
    })
}

/// A `length`-element array argument; `ptr` may be null only when `length` is 0.
pub(crate) fn slice<'a, T>(ptr: *const T, length: usize) -> Result<&'a [T], Status> {
    if length == 0 {
        return Ok(&[]);
    }
    if ptr.is_null() {
        return Err(Status::InvalidArg);
    }
    // SAFETY: the caller passes `length` readable elements at `ptr` (N-API contract).
    Ok(unsafe { std::slice::from_raw_parts(ptr, length) })
}

/// A writable `capacity`-element buffer; `None` when `ptr` is null.
pub(crate) fn slice_mut<'a, T>(ptr: *mut T, capacity: usize) -> Option<&'a mut [T]> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the caller passes `capacity` writable elements at `ptr` (N-API contract).
    Some(unsafe { std::slice::from_raw_parts_mut(ptr, capacity) })
}

/// A NUL-terminated name argument (`utf8name`, error codes and messages).
pub(crate) fn c_name<'a>(ptr: *const c_char) -> Result<&'a [u8], Status> {
    c_bytes(ptr, NAPI_AUTO_LENGTH)
}

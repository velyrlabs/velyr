//! Errors and exceptions. Throwing only records the env's pending exception; the trampoline
//! (or `load`) rethrows it once the addon returns to JS.

use std::ffi::c_char;

use crate::env::Env;
use crate::ffi::{c_bytes, c_name, local, local_as, napi_fn, napi_scope, to_napi, write};
use crate::types::{Status, napi_value};

pub(crate) type ErrorConstructor =
    for<'s> fn(&v8::PinScope<'s, '_>, v8::Local<'s, v8::String>) -> v8::Local<'s, v8::Value>;

/// `new <kind>(message)` with an optional `code` property.
fn make_error<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    kind: ErrorConstructor,
    code: Option<v8::Local<'s, v8::Value>>,
    message: v8::Local<'s, v8::String>,
) -> Result<v8::Local<'s, v8::Value>, Status> {
    let error = kind(scope, message);
    if let Some(code) = code {
        let object = error.to_object(scope).ok_or(Status::GenericFailure)?;
        let key = v8::String::new(scope, "code").ok_or(Status::GenericFailure)?;
        object
            .set(scope, key.into(), code)
            .ok_or(Status::GenericFailure)?;
    }
    Ok(error)
}

fn create(
    env: &Env,
    kind: ErrorConstructor,
    code: napi_value,
    message: napi_value,
    result: *mut napi_value,
) -> Result<(), Status> {
    napi_scope!(env, scope);
    let message = local_as::<v8::String>(message, Status::StringExpected)?;
    let code = if code.is_null() {
        None
    } else {
        Some(local_as::<v8::String>(code, Status::StringExpected)?.into())
    };
    write(result, to_napi(make_error(scope, kind, code, message)?))
}

pub(crate) fn throw(
    env: &Env,
    kind: ErrorConstructor,
    code: *const c_char,
    message: *const c_char,
) -> Result<(), Status> {
    napi_scope!(env, scope);
    let text = |bytes| {
        v8::String::new_from_utf8(scope, bytes, v8::NewStringType::Normal)
            .ok_or(Status::GenericFailure)
    };
    let message = text(c_name(message)?)?;
    let code = if code.is_null() {
        None
    } else {
        Some(text(c_name(code)?)?.into())
    };
    let error = make_error(scope, kind, code, message)?;
    env.set_pending_exception(scope, error);
    Ok(())
}

napi_fn! {
    fn napi_create_error(env, code: napi_value, message: napi_value, result: *mut napi_value) {
        create(env, v8::Exception::error, code, message, result)
    }
}

napi_fn! {
    fn napi_create_type_error(env, code: napi_value, message: napi_value, result: *mut napi_value) {
        create(env, v8::Exception::type_error, code, message, result)
    }
}

napi_fn! {
    fn napi_create_range_error(env, code: napi_value, message: napi_value, result: *mut napi_value) {
        create(env, v8::Exception::range_error, code, message, result)
    }
}

napi_fn! {
    fn napi_throw(env, error: napi_value) {
        napi_scope!(env, scope);
        env.set_pending_exception(scope, local(error)?);
        Ok(())
    }
}

napi_fn! {
    fn napi_throw_error(env, code: *const c_char, message: *const c_char) {
        throw(env, v8::Exception::error, code, message)
    }
}

napi_fn! {
    fn napi_throw_type_error(env, code: *const c_char, message: *const c_char) {
        throw(env, v8::Exception::type_error, code, message)
    }
}

napi_fn! {
    fn napi_throw_range_error(env, code: *const c_char, message: *const c_char) {
        throw(env, v8::Exception::range_error, code, message)
    }
}

napi_fn! {
    fn napi_is_error(env, value: napi_value, result: *mut bool) {
        write(result, local(value)?.is_native_error())
    }
}

napi_fn! {
    fn napi_is_exception_pending(env, result: *mut bool) {
        write(result, env.has_pending_exception())
    }
}

napi_fn! {
    fn napi_get_and_clear_last_exception(env, result: *mut napi_value) {
        napi_scope!(env, scope);
        let exception = env.take_pending_exception(scope).unwrap_or_else(|| v8::undefined(scope).into());
        write(result, to_napi(exception))
    }
}

/// `napi_fatal_error`: prints the location and message, then aborts.
#[unsafe(no_mangle)]
pub extern "C" fn napi_fatal_error(
    location: *const c_char,
    location_len: usize,
    message: *const c_char,
    message_len: usize,
) -> ! {
    let text = |ptr: *const c_char, len| {
        if ptr.is_null() {
            String::new()
        } else {
            c_bytes(ptr, len)
                .map(String::from_utf8_lossy)
                .unwrap_or_default()
                .into_owned()
        }
    };
    eprintln!(
        "FATAL ERROR: {} {}",
        text(location, location_len),
        text(message, message_len)
    );
    std::process::abort()
}

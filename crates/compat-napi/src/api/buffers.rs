//! ArrayBuffer, TypedArray and Buffer.

use std::ffi::c_void;

use crate::api::errors::throw;
use crate::env::{Env, Finalizer};
use crate::ffi::{local, local_as, napi_fn, napi_scope, slice, to_napi, write};
use crate::types::{Status, napi_finalize, napi_typedarray_type, napi_value};

fn write_data(out: *mut *mut c_void, data: *mut c_void) -> Result<(), Status> {
    if out.is_null() {
        Ok(())
    } else {
        write(out, data)
    }
}

fn buffer_data(buffer: v8::Local<'_, v8::ArrayBuffer>) -> *mut c_void {
    buffer
        .data()
        .map_or(std::ptr::null_mut(), |data| data.as_ptr())
}

/// A zero-filled ArrayBuffer holding a copy of `bytes` when given.
fn new_array_buffer<'s>(
    scope: &v8::PinScope<'s, '_>,
    length: usize,
    bytes: Option<&[u8]>,
) -> v8::Local<'s, v8::ArrayBuffer> {
    let buffer = v8::ArrayBuffer::new(scope, length);
    if let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) {
        // SAFETY: the fresh buffer has `length == bytes.len()` writable bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer_data(buffer).cast(), bytes.len())
        };
    }
    buffer
}

napi_fn! {
    fn napi_create_arraybuffer(env, length: usize, data: *mut *mut c_void, result: *mut napi_value) {
        napi_scope!(env, scope);
        let buffer = new_array_buffer(scope, length, None);
        write_data(data, buffer_data(buffer))?;
        write(result, to_napi(buffer.into()))
    }
}

napi_fn! {
    fn napi_create_external_arraybuffer(
        env,
        data: *mut c_void,
        length: usize,
        finalize: napi_finalize,
        hint: *mut c_void,
        result: *mut napi_value,
    ) {
        napi_scope!(env, scope);
        let buffer = copy_external(env, scope, data, length, finalize, hint)?;
        write(result, to_napi(buffer.into()))
    }
}

/// External memory is copied and released at once: running the addon's finalizer from V8's
/// backing-store deleter (any thread, inside GC) would not be safe.
// shortcut: copies instead of aliasing; mutations through the original pointer are not seen. Use
// a backing store with a deleter that defers the finalizer to `drain_finalizers` if that matters.
fn copy_external<'s>(
    env: &Env,
    scope: &v8::PinScope<'s, '_>,
    data: *mut c_void,
    length: usize,
    finalize: napi_finalize,
    hint: *mut c_void,
) -> Result<v8::Local<'s, v8::ArrayBuffer>, Status> {
    let bytes = slice(data.cast_const().cast::<u8>(), length)?;
    let buffer = new_array_buffer(scope, length, Some(bytes));
    Finalizer {
        callback: finalize,
        data,
        hint,
    }
    .run(env);
    Ok(buffer)
}

napi_fn! {
    fn napi_get_arraybuffer_info(env, value: napi_value, data: *mut *mut c_void, length: *mut usize) {
        let buffer = local_as::<v8::ArrayBuffer>(value, Status::InvalidArg)?;
        write_data(data, buffer_data(buffer))?;
        if !length.is_null() {
            write(length, buffer.byte_length())?;
        }
        Ok(())
    }
}

napi_fn! {
    fn napi_is_arraybuffer(env, value: napi_value, result: *mut bool) {
        write(result, local(value)?.is_array_buffer())
    }
}

/// The `napi_typedarray_type` values, in order, with their element sizes.
const ELEMENT_SIZES: [usize; 11] = [1, 1, 1, 2, 2, 4, 4, 4, 8, 8, 8];

fn new_typed_array<'s>(
    scope: &v8::PinScope<'s, '_>,
    kind: napi_typedarray_type,
    buffer: v8::Local<'s, v8::ArrayBuffer>,
    offset: usize,
    length: usize,
) -> Option<v8::Local<'s, v8::Value>> {
    Some(match kind {
        0 => v8::Int8Array::new(scope, buffer, offset, length)?.into(),
        1 => v8::Uint8Array::new(scope, buffer, offset, length)?.into(),
        2 => v8::Uint8ClampedArray::new(scope, buffer, offset, length)?.into(),
        3 => v8::Int16Array::new(scope, buffer, offset, length)?.into(),
        4 => v8::Uint16Array::new(scope, buffer, offset, length)?.into(),
        5 => v8::Int32Array::new(scope, buffer, offset, length)?.into(),
        6 => v8::Uint32Array::new(scope, buffer, offset, length)?.into(),
        7 => v8::Float32Array::new(scope, buffer, offset, length)?.into(),
        8 => v8::Float64Array::new(scope, buffer, offset, length)?.into(),
        9 => v8::BigInt64Array::new(scope, buffer, offset, length)?.into(),
        10 => v8::BigUint64Array::new(scope, buffer, offset, length)?.into(),
        _ => return None,
    })
}

fn typed_array_kind(value: v8::Local<'_, v8::Value>) -> napi_typedarray_type {
    let checks = [
        value.is_int8_array(),
        value.is_uint8_array(),
        value.is_uint8_clamped_array(),
        value.is_int16_array(),
        value.is_uint16_array(),
        value.is_int32_array(),
        value.is_uint32_array(),
        value.is_float32_array(),
        value.is_float64_array(),
        value.is_big_int64_array(),
        value.is_big_uint64_array(),
    ];
    checks
        .iter()
        .position(|matched| *matched)
        .map_or(-1, |kind| kind as napi_typedarray_type)
}

napi_fn! {
    fn napi_create_typedarray(
        env,
        kind: napi_typedarray_type,
        length: usize,
        buffer: napi_value,
        offset: usize,
        result: *mut napi_value,
    ) {
        let element = usize::try_from(kind).ok().and_then(|kind| ELEMENT_SIZES.get(kind)).ok_or(Status::InvalidArg)?;
        let buffer = local_as::<v8::ArrayBuffer>(buffer, Status::InvalidArg)?;
        // Node's checks and messages; both leave a RangeError pending.
        if !offset.is_multiple_of(*element) {
            throw(env, v8::Exception::range_error, c"ERR_NAPI_INVALID_TYPEDARRAY_ALIGNMENT".as_ptr(),
                c"start offset of the typed array should be a multiple of its element size".as_ptr())?;
            return Err(Status::PendingException);
        }
        let end = length.checked_mul(*element).and_then(|bytes| bytes.checked_add(offset));
        if end.is_none_or(|end| end > buffer.byte_length()) {
            throw(env, v8::Exception::range_error, c"ERR_NAPI_INVALID_TYPEDARRAY_LENGTH".as_ptr(),
                c"Invalid typed array length".as_ptr())?;
            return Err(Status::PendingException);
        }
        napi_scope!(env, scope);
        let array = new_typed_array(scope, kind, buffer, offset, length).ok_or(Status::GenericFailure)?;
        write(result, to_napi(array))
    }
}

napi_fn! {
    fn napi_get_typedarray_info(
        env,
        value: napi_value,
        kind: *mut napi_typedarray_type,
        length: *mut usize,
        data: *mut *mut c_void,
        buffer: *mut napi_value,
        offset: *mut usize,
    ) {
        napi_scope!(env, scope);
        let array = local_as::<v8::TypedArray>(value, Status::InvalidArg)?;
        let backing = array.buffer(scope).ok_or(Status::GenericFailure)?;
        let start = array.byte_offset();
        if !kind.is_null() {
            write(kind, typed_array_kind(array.into()))?;
        }
        if !length.is_null() {
            write(length, array.length())?;
        }
        let base = buffer_data(backing);
        // SAFETY: `start` is within the backing store the view was created over.
        write_data(data, if base.is_null() { base } else { unsafe { base.cast::<u8>().add(start).cast() } })?;
        if !buffer.is_null() {
            write(buffer, to_napi(backing.into()))?;
        }
        if !offset.is_null() {
            write(offset, start)?;
        }
        Ok(())
    }
}

napi_fn! {
    fn napi_is_typedarray(env, value: napi_value, result: *mut bool) {
        write(result, local(value)?.is_typed_array())
    }
}

/// A Uint8Array with `globalThis.Buffer.prototype` when the runtime installed one.
fn new_buffer<'s>(
    env: &Env,
    scope: &mut v8::PinScope<'s, '_>,
    backing: v8::Local<'s, v8::ArrayBuffer>,
) -> Result<v8::Local<'s, v8::Value>, Status> {
    let length = backing.byte_length();
    let array = v8::Uint8Array::new(scope, backing, 0, length).ok_or(Status::GenericFailure)?;
    let global = scope.get_current_context().global(scope);
    let prototype = env.run_js(scope, |scope| {
        let buffer = global.get(scope, v8::String::new(scope, "Buffer")?.into())?;
        let Ok(buffer) = v8::Local::<v8::Function>::try_from(buffer) else {
            return Some(None);
        };
        let prototype = buffer.get(scope, v8::String::new(scope, "prototype")?.into())?;
        Some(Some(v8::Global::new(scope, prototype)))
    })?;
    if let Some(prototype) = prototype {
        let prototype = v8::Local::new(scope, prototype);
        array
            .set_prototype(scope, prototype)
            .ok_or(Status::GenericFailure)?;
    }
    Ok(array.into())
}

napi_fn! {
    fn napi_create_buffer(env, length: usize, data: *mut *mut c_void, result: *mut napi_value) {
        napi_scope!(env, scope);
        let backing = new_array_buffer(scope, length, None);
        write_data(data, buffer_data(backing))?;
        write(result, to_napi(new_buffer(env, scope, backing)?))
    }
}

napi_fn! {
    fn napi_create_buffer_copy(
        env,
        length: usize,
        source: *const c_void,
        data: *mut *mut c_void,
        result: *mut napi_value,
    ) {
        napi_scope!(env, scope);
        let backing = new_array_buffer(scope, length, Some(slice(source.cast::<u8>(), length)?));
        write_data(data, buffer_data(backing))?;
        write(result, to_napi(new_buffer(env, scope, backing)?))
    }
}

napi_fn! {
    fn napi_create_external_buffer(
        env,
        length: usize,
        data: *mut c_void,
        finalize: napi_finalize,
        hint: *mut c_void,
        result: *mut napi_value,
    ) {
        napi_scope!(env, scope);
        let backing = copy_external(env, scope, data, length, finalize, hint)?;
        write(result, to_napi(new_buffer(env, scope, backing)?))
    }
}

napi_fn! {
    fn napi_get_buffer_info(env, value: napi_value, data: *mut *mut c_void, length: *mut usize) {
        let view = local_as::<v8::ArrayBufferView>(value, Status::InvalidArg)?;
        write_data(data, view.data())?;
        if !length.is_null() {
            write(length, view.byte_length())?;
        }
        Ok(())
    }
}

napi_fn! {
    fn napi_is_buffer(env, value: napi_value, result: *mut bool) {
        // As in Node, any ArrayBufferView is accepted where a Buffer is expected.
        write(result, local(value)?.is_array_buffer_view())
    }
}

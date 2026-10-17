//! UTF-8, UTF-16 and Latin-1 strings.

use std::ffi::c_char;

use crate::ffi::{
    c_bytes, local_as, napi_fn, napi_scope, slice, slice_mut, to_napi, write, write_opt,
};
use crate::types::{NAPI_AUTO_LENGTH, Status, napi_value};

napi_fn! {
    fn napi_create_string_utf8(env, text: *const c_char, length: usize, result: *mut napi_value) {
        napi_scope!(env, scope);
        let bytes = c_bytes(text, length)?;
        let string = v8::String::new_from_utf8(scope, bytes, v8::NewStringType::Normal)
            .ok_or(Status::GenericFailure)?;
        write(result, to_napi(string.into()))
    }
}

napi_fn! {
    fn napi_create_string_latin1(env, text: *const c_char, length: usize, result: *mut napi_value) {
        napi_scope!(env, scope);
        let bytes = c_bytes(text, length)?;
        let string = v8::String::new_from_one_byte(scope, bytes, v8::NewStringType::Normal)
            .ok_or(Status::GenericFailure)?;
        write(result, to_napi(string.into()))
    }
}

napi_fn! {
    fn napi_create_string_utf16(env, text: *const u16, length: usize, result: *mut napi_value) {
        napi_scope!(env, scope);
        let units = utf16_units(text, length)?;
        let string = v8::String::new_from_two_byte(scope, units, v8::NewStringType::Normal)
            .ok_or(Status::GenericFailure)?;
        write(result, to_napi(string.into()))
    }
}

fn utf16_units<'a>(text: *const u16, length: usize) -> Result<&'a [u16], Status> {
    if length != NAPI_AUTO_LENGTH {
        return if length > i32::MAX as usize {
            Err(Status::InvalidArg)
        } else {
            slice(text, length)
        };
    }
    if text.is_null() {
        return Err(Status::InvalidArg);
    }
    let mut end = 0;
    // SAFETY: with NAPI_AUTO_LENGTH the caller passes a NUL-terminated UTF-16 string.
    while unsafe { *text.add(end) } != 0 {
        end += 1;
    }
    slice(text, end)
}

fn string(value: napi_value) -> Result<v8::Local<'static, v8::String>, Status> {
    local_as::<v8::String>(value, Status::StringExpected)
}

/// The shared contract of the `napi_get_value_string_*` family: a null `buf` asks for the
/// length; otherwise up to `bufsize - 1` units are copied and NUL-terminated.
fn read_into<T: Copy + Default>(
    buf: *mut T,
    bufsize: usize,
    result: *mut usize,
    full_length: impl FnOnce() -> usize,
    copy: impl FnOnce(&mut [T]) -> usize,
) -> Result<(), Status> {
    let Some(buffer) = slice_mut(buf, bufsize) else {
        return write(result, full_length());
    };
    let Some(last) = bufsize.checked_sub(1) else {
        write_opt(result, 0);
        return Ok(());
    };
    let copied = copy(&mut buffer[..last]);
    buffer[copied] = T::default();
    write_opt(result, copied);
    Ok(())
}

napi_fn! {
    fn napi_get_value_string_utf8(
        env,
        value: napi_value,
        buf: *mut c_char,
        bufsize: usize,
        result: *mut usize,
    ) {
        napi_scope!(env, scope);
        let string = string(value)?;
        read_into(buf.cast::<u8>(), bufsize, result, || string.utf8_length(scope), |out| {
            // Writes whole characters only; lone surrogates become U+FFFD.
            string.write_utf8_v2(scope, out, v8::WriteFlags::kReplaceInvalidUtf8, None)
        })
    }
}

napi_fn! {
    fn napi_get_value_string_latin1(
        env,
        value: napi_value,
        buf: *mut c_char,
        bufsize: usize,
        result: *mut usize,
    ) {
        napi_scope!(env, scope);
        let string = string(value)?;
        read_into(buf.cast::<u8>(), bufsize, result, || string.length(), |out| {
            let copied = out.len().min(string.length());
            string.write_one_byte_v2(scope, 0, &mut out[..copied], v8::WriteFlags::empty());
            copied
        })
    }
}

napi_fn! {
    fn napi_get_value_string_utf16(
        env,
        value: napi_value,
        buf: *mut u16,
        bufsize: usize,
        result: *mut usize,
    ) {
        napi_scope!(env, scope);
        let string = string(value)?;
        read_into(buf, bufsize, result, || string.length(), |out| {
            let copied = out.len().min(string.length());
            string.write_v2(scope, 0, &mut out[..copied], v8::WriteFlags::empty());
            copied
        })
    }
}

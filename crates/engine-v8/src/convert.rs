//! `Value` <-> `v8::Local` conversion and exception mapping.

use std::sync::atomic::Ordering;

use velyr_engine_api::{JsBigInt, JsException, JsString, Value};

use crate::handles::HandleTable;
use crate::shared::Shared;

pub(crate) fn internal_exception(message: impl Into<String>) -> JsException {
    JsException {
        message: message.into(),
        stack: None,
        value: None,
    }
}

pub(crate) fn to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    handles: &HandleTable,
    value: &Value,
) -> Result<v8::Local<'s, v8::Value>, JsException> {
    Ok(match value {
        Value::Undefined => v8::undefined(scope).into(),
        Value::Null => v8::null(scope).into(),
        Value::Bool(b) => v8::Boolean::new(scope, *b).into(),
        Value::Number(n) => v8::Number::new(scope, *n).into(),
        Value::String(s) => match s.as_latin1() {
            Some(bytes) => v8::String::new_from_one_byte(scope, bytes, v8::NewStringType::Normal),
            None => v8::String::new_from_two_byte(scope, s.as_utf16(), v8::NewStringType::Normal),
        }
        .ok_or_else(|| internal_exception("string too long"))?
        .into(),
        Value::BigInt(b) => {
            let words = magnitude_to_words(b.magnitude_le());
            v8::BigInt::new_from_words(scope, b.is_negative(), &words)
                .ok_or_else(|| internal_exception("invalid BigInt"))?
                .into()
        }
        Value::Object(handle) => {
            let global = handles
                .get(*handle)
                .ok_or_else(|| internal_exception("stale or invalid handle"))?;
            v8::Local::new(scope, global)
        }
    })
}

pub(crate) fn from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    handles: &mut HandleTable,
    value: v8::Local<'_, v8::Value>,
) -> Value {
    if value.is_undefined() {
        Value::Undefined
    } else if value.is_null() {
        Value::Null
    } else if value.is_boolean() {
        Value::Bool(value.is_true())
    } else if value.is_number() {
        Value::Number(value.number_value(scope).unwrap_or(f64::NAN))
    } else if let Ok(string) = v8::Local::<v8::String>::try_from(value) {
        Value::String(string_to_js(scope, string))
    } else if let Ok(bigint) = v8::Local::<v8::BigInt>::try_from(value) {
        Value::BigInt(bigint_to_js(bigint))
    } else {
        Value::Object(handles.alloc(v8::Global::new(scope, value)))
    }
}

fn string_to_js(scope: &mut v8::PinScope<'_, '_>, string: v8::Local<'_, v8::String>) -> JsString {
    if string.is_onebyte() {
        let mut bytes = vec![0u8; string.length()];
        string.write_one_byte_v2(scope, 0, &mut bytes, v8::WriteFlags::empty());
        return JsString::from_latin1(bytes);
    }
    let mut units = vec![0u16; string.length()];
    string.write_v2(scope, 0, &mut units, v8::WriteFlags::empty());
    JsString::from_utf16(units)
}

fn bigint_to_js(bigint: v8::Local<'_, v8::BigInt>) -> JsBigInt {
    let mut words = vec![0u64; bigint.word_count()];
    let (negative, words) = bigint.to_words_array(&mut words);
    let magnitude = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    JsBigInt::new(negative, magnitude)
}

fn magnitude_to_words(magnitude: &[u8]) -> Vec<u64> {
    magnitude
        .chunks(8)
        .map(|chunk| {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            u64::from_le_bytes(word)
        })
        .collect()
}

/// Maps the exception caught by `tc` to a [`JsException`]. Sets `terminated`
/// when it came from `terminate_execution`.
pub(crate) fn exception_from(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    shared: &Shared,
) -> JsException {
    if tc.has_terminated() {
        shared.terminated.set(true);
        if shared.heap_exhausted.swap(false, Ordering::Acquire) {
            return JsException::quota_exceeded("heap");
        }
        return internal_exception("execution terminated");
    }
    let Some(exception) = tc.exception() else {
        return internal_exception("unknown exception");
    };
    // No handle borrow is held here: `to_string` may run JS.
    let message = exception
        .to_string(tc)
        .map(|s| s.to_rust_string_lossy(tc))
        .unwrap_or_else(|| "uncaught exception".to_owned());
    let stack = tc
        .stack_trace()
        .and_then(|s| s.to_string(tc))
        .map(|s| s.to_rust_string_lossy(tc))
        .filter(|s| !s.is_empty());
    let value = from_v8(tc, &mut shared.handles.borrow_mut(), exception);
    JsException {
        message,
        stack,
        value: Some(value),
    }
}

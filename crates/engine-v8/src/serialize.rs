//! Structured clone on `v8::ValueSerializer` / `ValueDeserializer`.
//!
//! Both entry points run inside a `TryCatch`: every failure leaves a JS exception pending
//! in it (a `DataCloneError` when the engine did not throw one itself), so callers map it
//! with `exception_from` or `caught`.

use v8::{ValueDeserializerHelper as _, ValueSerializerHelper as _};
use velyr_engine_api::{SerializedValue, TransferredBuffer, Value};

use crate::convert::{from_v8, to_v8};
use crate::shared::Shared;

/// `DOMException` code of `DataCloneError`.
const DATA_CLONE_ERR: f64 = 25.0;

/// An `ArrayBuffer` backing store in flight between isolates.
struct SendStore(v8::SharedRef<v8::BackingStore>);

// SAFETY: V8 backing stores are reference counted atomically and `BackingStore` is `Send`; the
// store is only reachable through this wrapper, and `TransferredBuffer::take_payload` moves it
// out on the first deserialize, so at most one isolate ever receives it.
#[allow(unsafe_code)]
unsafe impl Send for SendStore {}

struct Delegate;

impl v8::ValueSerializerImpl for Delegate {
    fn throw_data_clone_error<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        message: v8::Local<'s, v8::String>,
    ) {
        let error = data_clone_error(scope, message);
        scope.throw_exception(error);
    }
}

impl v8::ValueDeserializerImpl for Delegate {}

/// `Error` named `DataCloneError` with `code` 25 (no `DOMException` yet).
fn data_clone_error<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    message: v8::Local<'s, v8::String>,
) -> v8::Local<'s, v8::Value> {
    let error = v8::Exception::error(scope, message);
    if let Ok(object) = v8::Local::<v8::Object>::try_from(error) {
        let name = v8::String::new(scope, "DataCloneError").map(Into::into);
        let code = Some(v8::Number::new(scope, DATA_CLONE_ERR).into());
        for (key, value) in [("name", name), ("code", code)] {
            if let (Some(key), Some(value)) = (v8::String::new(scope, key), value) {
                // Cannot fail on a fresh, extensible Error object.
                let _ = object.define_own_property(
                    scope,
                    key.into(),
                    value,
                    v8::PropertyAttribute::DONT_ENUM,
                );
            }
        }
    }
    error
}

fn fail<T>(scope: &mut v8::PinScope<'_, '_>, message: &str) -> Option<T> {
    let message = v8::String::new(scope, message).unwrap_or_else(|| v8::String::empty(scope));
    let error = data_clone_error(scope, message);
    scope.throw_exception(error);
    None
}

pub(crate) fn serialize(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    shared: &Shared,
    value: &Value,
    transfer: &[Value],
) -> Option<SerializedValue> {
    let Ok(value) = to_v8(tc, &shared.handles.borrow(), value) else {
        return fail(tc, "Invalid value");
    };
    let mut buffers: Vec<v8::Local<v8::ArrayBuffer>> = Vec::with_capacity(transfer.len());
    for item in transfer {
        let Ok(item) = to_v8(tc, &shared.handles.borrow(), item) else {
            return fail(tc, "Invalid transfer list entry");
        };
        let Ok(buffer) = v8::Local::<v8::ArrayBuffer>::try_from(item) else {
            return fail(tc, "Value not transferable");
        };
        if buffers.contains(&buffer) {
            return fail(tc, "ArrayBuffer is listed twice in the transfer list");
        }
        if buffer.was_detached() || !buffer.is_detachable() {
            return fail(tc, "ArrayBuffer is detached or not transferable");
        }
        buffers.push(buffer);
    }
    let context = tc.get_current_context();
    let serializer = v8::ValueSerializer::new(tc, Box::new(Delegate));
    serializer.write_header();
    for (id, buffer) in buffers.iter().enumerate() {
        serializer.transfer_array_buffer(id as u32, *buffer);
    }
    shared.microtasks_dirty.set(true);
    if serializer.write_value(context, value) != Some(true) {
        return if tc.has_caught() {
            None
        } else {
            fail(tc, "Value could not be cloned")
        };
    }
    // Stores are taken before detaching: afterwards the buffers no longer expose them.
    let transferred = buffers
        .iter()
        .map(|buffer| {
            let store = buffer.get_backing_store();
            buffer.detach(None);
            TransferredBuffer::new(SendStore(store))
        })
        .collect();
    Some(SerializedValue {
        bytes: serializer.release(),
        transferred,
    })
}

pub(crate) fn deserialize(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    shared: &Shared,
    serialized: &SerializedValue,
) -> Option<Value> {
    let context = tc.get_current_context();
    let deserializer = v8::ValueDeserializer::new(tc, Box::new(Delegate), &serialized.bytes);
    if deserializer.read_header(context) != Some(true) {
        return if tc.has_caught() {
            None
        } else {
            fail(tc, "Serialized data is corrupt")
        };
    }
    for (id, transferred) in serialized.transferred.iter().enumerate() {
        let Some(store) = transferred.take_payload::<SendStore>() else {
            return fail(tc, "Transferred buffer was already consumed or is foreign");
        };
        let buffer = v8::ArrayBuffer::with_backing_store(tc, &store.0);
        deserializer.transfer_array_buffer(id as u32, buffer);
    }
    shared.microtasks_dirty.set(true);
    let Some(value) = deserializer.read_value(context) else {
        return if tc.has_caught() {
            None
        } else {
            fail(tc, "Serialized data is corrupt")
        };
    };
    Some(from_v8(tc, &mut shared.handles.borrow_mut(), value))
}

//! Structured clone on `v8::ValueSerializer` / `ValueDeserializer`.
//!
//! Both entry points run inside a `TryCatch`: every failure leaves a JS exception pending
//! in it (a `DataCloneError` when the engine did not throw one itself), so callers map it
//! with `exception_from` or `caught`.

use std::cell::RefCell;
use std::rc::Rc;

use v8::{ValueDeserializerHelper as _, ValueSerializerHelper as _};
use velyr_engine_api::{SerializedValue, TransferredBuffer, Value};

use crate::convert::{from_v8, to_v8};
use crate::shared::Shared;

/// `DOMException` code of `DataCloneError`.
const DATA_CLONE_ERR: f64 = 25.0;

/// Present in `Shared::slots` when the engine was built with `V8Engine::with_shared_memory`.
pub(crate) struct SharedMemoryAllowed;

fn shared_memory_allowed(shared: &Shared) -> bool {
    let slots = shared.slots.borrow();
    slots.contains_key(&std::any::TypeId::of::<SharedMemoryAllowed>())
}

/// A backing store in flight between isolates. `shared` marks a `SharedArrayBuffer` store,
/// which stays alive (and writable) in the sender too; an `ArrayBuffer` store was detached.
struct SendStore {
    store: v8::SharedRef<v8::BackingStore>,
    shared: bool,
}

// SAFETY: V8 backing stores are reference counted atomically and `BackingStore` is `Send`; the
// store is only reachable through this wrapper, and `TransferredBuffer::take_payload` moves it
// out on the first deserialize, so at most one isolate ever receives it. A shared store is
// meant to be accessed from several isolates at once (SharedArrayBuffer semantics, Atomics);
// it is only ever put in the wrapper when `with_shared_memory` opted in.
#[allow(unsafe_code)]
unsafe impl Send for SendStore {}

/// Per-call state shared between the entry points and the V8 delegate.
#[derive(Default)]
struct SabState {
    allowed: bool,
    /// Serializer: ids of the stores in `outgoing` start here (after the ArrayBuffer ids).
    first_id: u32,
    outgoing: RefCell<Vec<v8::SharedRef<v8::BackingStore>>>,
    /// Deserializer: indexed by transfer id, `Some` for `SharedArrayBuffer` stores.
    incoming: Vec<Option<v8::SharedRef<v8::BackingStore>>>,
}

struct Delegate(Rc<SabState>);

impl v8::ValueSerializerImpl for Delegate {
    fn get_shared_array_buffer_id<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        buffer: v8::Local<'s, v8::SharedArrayBuffer>,
    ) -> Option<u32> {
        if !self.0.allowed {
            let message = v8::String::new(scope, "#<SharedArrayBuffer> could not be cloned.")?;
            self.throw_data_clone_error(scope, message);
            return None;
        }
        let mut outgoing = self.0.outgoing.borrow_mut();
        outgoing.push(buffer.get_backing_store());
        Some(self.0.first_id + outgoing.len() as u32 - 1)
    }

    fn throw_data_clone_error<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        message: v8::Local<'s, v8::String>,
    ) {
        let error = data_clone_error(scope, message);
        scope.throw_exception(error);
    }
}

impl v8::ValueDeserializerImpl for Delegate {
    fn get_shared_array_buffer_from_id<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        transfer_id: u32,
    ) -> Option<v8::Local<'s, v8::SharedArrayBuffer>> {
        let store = self.0.incoming.get(transfer_id as usize)?.as_ref()?;
        Some(v8::SharedArrayBuffer::with_backing_store(scope, store))
    }
}

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
    let state = Rc::new(SabState {
        allowed: shared_memory_allowed(shared),
        first_id: buffers.len() as u32,
        ..SabState::default()
    });
    let serializer = v8::ValueSerializer::new(tc, Box::new(Delegate(Rc::clone(&state))));
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
    let mut transferred: Vec<TransferredBuffer> = buffers
        .iter()
        .map(|buffer| {
            let store = buffer.get_backing_store();
            buffer.detach(None);
            TransferredBuffer::new(SendStore {
                store,
                shared: false,
            })
        })
        .collect();
    transferred.extend(state.outgoing.take().into_iter().map(|store| {
        TransferredBuffer::new(SendStore {
            store,
            shared: true,
        })
    }));
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
    let mut stores = Vec::with_capacity(serialized.transferred.len());
    for transferred in &serialized.transferred {
        let Some(store) = transferred.take_payload::<SendStore>() else {
            return fail(tc, "Transferred buffer was already consumed or is foreign");
        };
        stores.push(store);
    }
    if !shared_memory_allowed(shared) && stores.iter().any(|store| store.shared) {
        return fail(
            tc,
            "SharedArrayBuffer transfer is not enabled for this isolate",
        );
    }
    let mut state = SabState::default();
    state.incoming = stores
        .iter()
        .map(|store| store.shared.then(|| store.store.clone()))
        .collect();
    let deserializer =
        v8::ValueDeserializer::new(tc, Box::new(Delegate(Rc::new(state))), &serialized.bytes);
    if deserializer.read_header(context) != Some(true) {
        return if tc.has_caught() {
            None
        } else {
            fail(tc, "Serialized data is corrupt")
        };
    }
    for (id, store) in stores.iter().enumerate().filter(|(_, store)| !store.shared) {
        let buffer = v8::ArrayBuffer::with_backing_store(tc, &store.store);
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

//! Structured serialization (HTML structured clone) as an engine-agnostic, `Send` payload.

use std::any::Any;
use std::cell::Cell;
use std::fmt;

use crate::JsException;

/// An `ArrayBuffer` whose backing store was moved out of its isolate. The payload is
/// opaque: only the adapter that produced it can read it back, and only once.
pub struct TransferredBuffer(Cell<Option<Box<dyn Any + Send>>>);

impl TransferredBuffer {
    pub fn new(payload: impl Any + Send) -> Self {
        Self(Cell::new(Some(Box::new(payload))))
    }

    /// Moves the adapter payload out, leaving the buffer consumed. `None` when it was already
    /// taken or came from a different adapter.
    pub fn take_payload<T: Any>(&self) -> Option<T> {
        self.0.take()?.downcast().ok().map(|payload| *payload)
    }
}

impl fmt::Debug for TransferredBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TransferredBuffer")
    }
}

/// A value flattened to bytes plus the buffers moved with it. Can cross threads and be
/// deserialized in any isolate of the same adapter.
#[derive(Debug)]
pub struct SerializedValue {
    pub bytes: Vec<u8>,
    /// Indexed by the transfer id embedded in `bytes`.
    pub transferred: Vec<TransferredBuffer>,
}

impl JsException {
    /// What an adapter without [`EngineFeatures::SERIALIZE`](crate::EngineFeatures::SERIALIZE) returns.
    pub fn serialize_unsupported() -> Self {
        Self {
            message: "structured serialization is not supported by this engine".into(),
            stack: None,
            value: None,
        }
    }
}

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<SerializedValue>();
};

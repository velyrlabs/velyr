//! Binary data built-ins (ECMA-262 §25.1-25.4, §23.2): ArrayBuffer, SharedArrayBuffer,
//! the TypedArrays (Float16Array and the BigInt ones included), DataView and Atomics.
//! Backing stores live outside the GC heap (see `store`); objects hold them in slots.

mod atomics;
mod base64;
mod buffer;
mod constructors;
mod data_view;
mod elements;
pub(crate) mod exotic;
mod methods;
pub(crate) mod store;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use super::abstract_ops::MAX_SAFE_INTEGER;
use super::slots::Slot;
use super::{Interpreter, Intrinsic, JsResult, object_value};
use crate::{ObjectRef, Value};
pub(crate) use elements::ElementKind;
use store::{Block, LocalBuffer, SharedBuffer, Store, View};

/// `[[TypedArrayName]]`/`[[ContentType]]` with the window on the buffer.
pub(crate) struct TypedArray {
    pub kind: ElementKind,
    pub view: View,
}

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    buffer::install(vm)?;
    constructors::install(vm)?;
    base64::install(vm)?;
    data_view::install(vm)?;
    atomics::install(vm)
}

impl Interpreter {
    /// ToIndex.
    pub(crate) fn coerce_index(&mut self, value: &Value) -> JsResult<usize> {
        let n = self.coerce_integer(value)?;
        if !(0.0..=MAX_SAFE_INTEGER).contains(&n) {
            return self.throw_range("Invalid index: not in the range 0 to 2^53 - 1");
        }
        Ok(n as usize)
    }

    /// An ArrayBuffer (or SharedArrayBuffer) of `len` zero bytes with `proto`, charged to
    /// the heap budget; a RangeError past it.
    pub(crate) fn create_buffer(
        &mut self,
        proto: ObjectRef,
        len: usize,
        max: Option<usize>,
        shared: bool,
    ) -> JsResult<ObjectRef> {
        let Some(block) = Block::new(&self.buffer_budget, len) else {
            return self.throw_range("Array buffer allocation failed");
        };
        let store = match shared {
            true => Store::Shared(Arc::new(SharedBuffer::new(block, max))),
            false => Store::Local(Rc::new(RefCell::new(LocalBuffer {
                block: Some(block),
                max,
            }))),
        };
        self.new_slotted(proto, Slot::ArrayBuffer(store))
    }

    /// A TypedArray object over `buffer`.
    pub(crate) fn create_typed_array(
        &mut self,
        proto: ObjectRef,
        kind: ElementKind,
        buffer: ObjectRef,
        byte_offset: usize,
        fixed: Option<usize>,
    ) -> JsResult<ObjectRef> {
        let Some(store) = self.buffer_store(&crate::interpreter::object_value(buffer)) else {
            return Err(self.internal("typed array over a non-buffer"));
        };
        let view = View {
            buffer,
            store,
            byte_offset,
            fixed,
        };
        let array = TypedArray { kind, view };
        self.new_slotted(proto, Slot::TypedArray(Rc::new(array)))
    }

    /// A new ArrayBuffer holding a copy of `bytes`, charged to the heap budget.
    pub fn create_array_buffer(&mut self, bytes: &[u8]) -> JsResult<Value> {
        let proto = self.intrinsic(Intrinsic::ArrayBufferPrototype);
        let buffer = self.create_buffer(proto, bytes.len(), None, false)?;
        let value = object_value(buffer);
        if let Some(store) = self.buffer_store(&value) {
            store.with_bytes(|target| target.copy_from_slice(bytes));
        }
        Ok(value)
    }

    /// Runs `f` on the bytes an ArrayBuffer, SharedArrayBuffer, TypedArray or DataView
    /// covers, without copying; `None` for other values and detached or out-of-bounds views.
    pub fn with_buffer_bytes<R>(&self, value: &Value, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        let (store, offset, len) = match self.value_slot(value)? {
            Slot::ArrayBuffer(store) if !store.is_detached() => {
                let len = store.byte_length();
                (store, 0, len)
            }
            Slot::TypedArray(array) => {
                let len = array.length()? * array.kind.size();
                (array.view.store.clone(), array.view.byte_offset, len)
            }
            Slot::DataView(view) => (view.store.clone(), view.byte_offset, view.length(1)?),
            _ => return None,
        };
        Some(store.with_bytes(|bytes| f(&bytes[offset..offset + len])))
    }

    /// `[[ArrayBufferData]]` of an ArrayBuffer or SharedArrayBuffer.
    pub(crate) fn buffer_store(&self, value: &Value) -> Option<Store> {
        match self.value_slot(value) {
            Some(Slot::ArrayBuffer(store)) => Some(store),
            _ => None,
        }
    }

    /// ValidateTypedArray: `this` is a TypedArray in bounds; returns it with its length.
    pub(crate) fn validate_typed_array(
        &mut self,
        this: &Value,
        method: &str,
    ) -> JsResult<(Rc<TypedArray>, usize)> {
        let Some(array) = self.typed_array_of(this) else {
            return self.throw_type(&format!("{method}: this is not a typed array"));
        };
        match array.length() {
            Some(length) => Ok((array, length)),
            None => self.throw_type(&format!(
                "{method}: the typed array is detached or out of bounds"
            )),
        }
    }
}

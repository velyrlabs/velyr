//! TypedArray exotic objects (ECMA-262 §10.4.5): numeric keys address the elements in the
//! backing store instead of ordinary properties. `exotic` is the one slot lookup the
//! internal methods do to pick proxy traps, element access or ordinary storage.
//! shortcut: named-property inline caches skip this, so `ta.Infinity`/`ta.NaN` written as
//! identifiers read ordinary properties; route cached sites here if that ever matters.

use std::rc::Rc;

use super::super::Interpreter;
use super::super::abstract_ops::MAX_SAFE_INTEGER;
use super::super::slots::Slot;
use super::elements::{read_raw, write_raw};
use super::{ElementKind, TypedArray};
use crate::interpreter::JsResult;
use crate::{JsStr, ObjectKey, ObjectRef, Property, PropertyDescriptor, PropertyValue, Value};

const NATIVE_LITTLE_ENDIAN: bool = cfg!(target_endian = "little");

pub(crate) enum Exotic {
    Ordinary,
    Proxy,
    /// A numeric key of a TypedArray; `None` for one that can never be an element index.
    Element(Rc<TypedArray>, Option<usize>),
}

impl TypedArray {
    /// `[[ArrayLength]]`; `None` when detached or out of bounds.
    pub(crate) fn length(&self) -> Option<usize> {
        self.view.length(self.kind.size())
    }

    /// The index when it is a valid integer index (IsValidIntegerIndex).
    pub(crate) fn valid(&self, index: Option<usize>) -> Option<usize> {
        index.filter(|&index| self.length().is_some_and(|length| index < length))
    }

    /// TypedArrayGetElement: `undefined` for an invalid index.
    pub(crate) fn read(&self, index: Option<usize>) -> Value {
        match self.valid(index) {
            Some(index) => self.kind.decode(self.load_raw(index)),
            None => Value::Undefined,
        }
    }

    pub(crate) fn load_raw(&self, index: usize) -> u64 {
        let (start, size) = self.byte_range(index);
        let bytes = |bytes: &mut [u8]| {
            bytes
                .get(start..start + size)
                .map(|b| read_raw(b, NATIVE_LITTLE_ENDIAN))
        };
        self.view.store.with_bytes(bytes).unwrap_or(0)
    }

    /// Writes the raw bits of element `index`; out of bounds writes are dropped.
    pub(crate) fn store_raw(&self, index: usize, raw: u64) {
        let (start, size) = self.byte_range(index);
        self.view.store.with_bytes(|bytes| {
            if let Some(dst) = bytes.get_mut(start..start + size) {
                write_raw(dst, raw, NATIVE_LITTLE_ENDIAN);
            }
        });
    }

    fn byte_range(&self, index: usize) -> (usize, usize) {
        let size = self.kind.size();
        (self.view.byte_offset + index * size, size)
    }
}

/// CanonicalNumericIndexString of a property name, as an element index.
fn canonical_numeric_index(name: &JsStr) -> Option<Option<usize>> {
    let text = name.to_string_lossy();
    if text == "-0" {
        return Some(None);
    }
    let first = text.chars().next()?;
    if !(first.is_ascii_digit() || matches!(first, '-' | 'I' | 'N')) {
        return None;
    }
    let n = crate::convert::string_to_number(name);
    if crate::number_to_string(n) != text {
        return None;
    }
    let is_index = n.fract() == 0.0 && (0.0..=MAX_SAFE_INTEGER).contains(&n);
    Some(is_index.then_some(n as usize))
}

impl Interpreter {
    /// How `o` handles `key`: proxy traps, a TypedArray element, or ordinary storage.
    pub(crate) fn exotic(&self, o: ObjectRef, key: ObjectKey) -> Exotic {
        match self.slot(o) {
            Some(Slot::Proxy(_)) => Exotic::Proxy,
            Some(Slot::TypedArray(array)) => {
                let index = match key {
                    ObjectKey::Index(index) => Some(Some(index as usize)),
                    ObjectKey::Name(atom) => {
                        canonical_numeric_index(self.realm.interner.resolve(atom))
                    }
                    ObjectKey::Symbol(_) => None,
                };
                match index {
                    Some(index) => Exotic::Element(array, index),
                    None => Exotic::Ordinary,
                }
            }
            _ => Exotic::Ordinary,
        }
    }

    pub(crate) fn typed_array_of(&self, value: &Value) -> Option<Rc<TypedArray>> {
        match self.value_slot(value) {
            Some(Slot::TypedArray(array)) => Some(array),
            _ => None,
        }
    }

    /// ToNumber, or ToBigInt for a BigInt element type.
    pub(crate) fn element_value(&mut self, kind: ElementKind, value: &Value) -> JsResult<Value> {
        match kind.is_bigint() {
            true => Ok(Value::BigInt(super::super::bigint::to_bigint(self, value)?)),
            false => Ok(Value::Number(self.coerce_number(value)?)),
        }
    }

    /// TypedArraySetElement: converts first (which may detach), then writes if still valid.
    pub(crate) fn typed_set(
        &mut self,
        array: &TypedArray,
        index: Option<usize>,
        value: &Value,
    ) -> JsResult<()> {
        let value = self.element_value(array.kind, value)?;
        if let Some(index) = array.valid(index) {
            array.store_raw(index, array.kind.encode(&value));
        }
        Ok(())
    }

    /// [[GetOwnProperty]] of an element.
    pub(crate) fn typed_own_property(array: &TypedArray, index: Option<usize>) -> Option<Property> {
        array.valid(index).map(|_| Property {
            value: PropertyValue::Data {
                value: array.read(index),
                writable: true,
            },
            enumerable: true,
            configurable: true,
        })
    }

    /// [[DefineOwnProperty]] of an element: only a writable, enumerable, configurable data
    /// property fits.
    pub(crate) fn typed_define(
        &mut self,
        array: &TypedArray,
        index: Option<usize>,
        desc: &PropertyDescriptor,
    ) -> JsResult<bool> {
        let fits = array.valid(index).is_some()
            && desc.configurable != Some(false)
            && desc.enumerable != Some(false)
            && desc.writable != Some(false)
            && desc.get.is_none()
            && desc.set.is_none();
        if !fits {
            return Ok(false);
        }
        if let Some(value) = &desc.value {
            self.typed_set(array, index, value)?;
        }
        Ok(true)
    }

    /// The element keys `0..length` that lead [[OwnPropertyKeys]].
    pub(crate) fn typed_keys(&mut self, array: &TypedArray) -> Vec<ObjectKey> {
        let length = array.length().unwrap_or(0);
        (0..length)
            .map(|index| self.index_key(index as f64))
            .collect()
    }
}

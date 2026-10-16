//! Array exotic objects (ECMA-262 §10.4.2): [[DefineOwnProperty]] and ArraySetLength.

use super::descriptor::validate_and_apply;
use super::{
    ObjectError, ObjectKey, ObjectKind, ObjectRef, ObjectResult, PropertyDescriptor, PropertyValue,
    Realm,
};
use crate::{TypeError, Value, to_number};

impl Realm {
    fn array_length(&self, o: ObjectRef) -> (u32, bool) {
        match self.object(o).kind {
            ObjectKind::Array {
                length,
                length_writable,
            } => (length, length_writable),
            _ => unreachable!("array_length on a non-array object"),
        }
    }

    fn store_array_length(&mut self, o: ObjectRef, length: u32, length_writable: bool) {
        self.object_mut(o).kind = ObjectKind::Array {
            length,
            length_writable,
        };
    }

    /// §10.4.2.1 [[DefineOwnProperty]].
    pub(super) fn array_define_own_property(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        desc: &PropertyDescriptor,
    ) -> ObjectResult<bool> {
        if key == self.length_key {
            return self.array_set_length(o, desc);
        }
        let ObjectKey::Index(index) = key else {
            return Ok(self.ordinary_define_own_property(o, key, desc));
        };
        let (length, length_writable) = self.array_length(o);
        if index >= length && !length_writable {
            return Ok(false);
        }
        if !self.ordinary_define_own_property(o, key, desc) {
            return Ok(false);
        }
        if index >= length {
            self.store_array_length(o, index + 1, length_writable);
        }
        Ok(true)
    }

    /// §10.4.2.4 ArraySetLength.
    fn array_set_length(&mut self, o: ObjectRef, desc: &PropertyDescriptor) -> ObjectResult<bool> {
        let Some(value) = &desc.value else {
            return Ok(self.define_length(o, desc));
        };
        let new_length = to_array_length(value)?;
        let mut new_desc = desc.clone();
        new_desc.value = Some(Value::Number(f64::from(new_length)));
        let (old_length, old_writable) = self.array_length(o);
        if new_length >= old_length {
            return Ok(self.define_length(o, &new_desc));
        }
        if !old_writable {
            return Ok(false);
        }
        // Writable is cleared only after the deletions, so a partial failure can still shrink length.
        let keeps_writable = new_desc.writable != Some(false);
        if !keeps_writable {
            new_desc.writable = Some(true);
        }
        if !self.define_length(o, &new_desc) {
            return Ok(false);
        }
        if let Err(blocked) = self.object_mut(o).elements.truncate(new_length) {
            new_desc.value = Some(Value::Number(f64::from(blocked) + 1.0));
            new_desc.writable = Some(keeps_writable);
            self.define_length(o, &new_desc);
            return Ok(false);
        }
        if !keeps_writable {
            let freeze = PropertyDescriptor {
                writable: Some(false),
                ..PropertyDescriptor::default()
            };
            self.define_length(o, &freeze);
        }
        Ok(true)
    }

    /// OrdinaryDefineOwnProperty(A, "length", desc) on the virtual `length` property;
    /// `desc.value`, when present, is already a valid array length.
    fn define_length(&mut self, o: ObjectRef, desc: &PropertyDescriptor) -> bool {
        let (length, writable) = self.array_length(o);
        let current =
            super::Property::data(Value::Number(f64::from(length)), writable, false, false);
        let extensible = self.object(o).extensible;
        let Some(updated) = validate_and_apply(Some(&current), desc, extensible) else {
            return false;
        };
        let PropertyValue::Data {
            value: Value::Number(length),
            writable,
        } = updated.value
        else {
            unreachable!("non-configurable length stays a numeric data property");
        };
        self.store_array_length(o, length as u32, writable);
        true
    }
}

/// ToUint32(value), throwing RangeError unless it equals ToNumber(value).
/// shortcut: objects are rejected instead of running valueOf/toString; wire the
/// interpreter's ToPrimitive in once it exists.
fn to_array_length(value: &Value) -> ObjectResult<u32> {
    let mut no_objects = |_, _| Err(TypeError("Cannot convert object to primitive value".into()));
    let number = to_number(value, &mut no_objects)?;
    let length = to_uint32(number);
    if f64::from(length) != number {
        return Err(ObjectError::Range("Invalid array length".into()));
    }
    Ok(length)
}

fn to_uint32(number: f64) -> u32 {
    const TWO_POW_32: f64 = 4_294_967_296.0;
    if !number.is_finite() {
        return 0;
    }
    number.trunc().rem_euclid(TWO_POW_32) as u32
}

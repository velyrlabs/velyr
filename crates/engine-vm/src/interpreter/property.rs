//! Property access. Named sites go through their inline cache first; on a miss the cache
//! falls back to the realm, whose accessor hook cannot run JS and reports
//! `NEEDS_INTERPRETER` instead, so the access is redone here where getters/setters can run.

use super::builtins::Intrinsic;
use super::builtins::typed_arrays::exotic::Exotic;
use super::class::is_private;
use super::state::CodeInfo;
use super::{ErrorKind, Interpreter, JsResult, object_value};
use crate::bytecode::{ConstIndex, Instruction as I, Reg};
use crate::{
    InternalMethods, JsStr, MAX_ARRAY_INDEX, ObjectError, ObjectKey, ObjectRef, ObjectResult,
    Property, PropertyDescriptor, PropertyKey, PropertyValue, Realm, Value, same_value,
};

const NEEDS_INTERPRETER: &str = "accessor call needs the interpreter";

/// `Realm::call` hook: the realm reached an accessor; the interpreter redoes the access.
pub(crate) fn accessor_needs_interpreter(
    _: &mut Realm,
    _: &Value,
    _: &Value,
    _: &[Value],
) -> ObjectResult<Value> {
    Err(ObjectError::Type(NEEDS_INTERPRETER.into()))
}

fn needs_interpreter(error: &ObjectError) -> bool {
    matches!(error, ObjectError::Type(message) if message == NEEDS_INTERPRETER)
}

impl Interpreter {
    pub(crate) fn step_property(&mut self, info: &CodeInfo, instruction: &I) -> JsResult<()> {
        match instruction {
            I::GetNamed {
                object,
                name,
                cache,
            } => {
                let key = self.name_key(info, *name);
                let base = self.reg(*object).clone();
                self.state.acc = self.get_value(&base, key, Some((info, *cache)))?;
            }
            I::GetKeyed { object } => {
                let key = self.object_key(&self.state.acc.clone())?;
                let base = self.reg(*object).clone();
                self.state.acc = self.get_value(&base, key, None)?;
            }
            I::SetNamed {
                object,
                name,
                cache,
            } => {
                let key = self.name_key(info, *name);
                self.set_value(*object, key, Some((info, *cache)))?;
            }
            I::SetKeyed { object, key } => {
                // Key conversion may run JS (toString/valueOf), which clobbers the accumulator.
                let value = self.state.acc.clone();
                let mark = self.state.scratch.len();
                self.keep(&value);
                let key = self.object_key(&self.reg(*key).clone());
                self.state.scratch.truncate(mark);
                self.state.acc = value;
                self.set_value(*object, key?, None)?;
            }
            I::DeleteProperty { object } => {
                let key = self.object_key(&self.state.acc.clone())?;
                let base = self.reg(*object).clone();
                self.state.acc = Value::Bool(self.delete_property(&base, key)?);
            }
            other => return self.step_literal(info, other),
        }
        Ok(())
    }

    pub(crate) fn as_object(&self, value: &Value) -> Option<ObjectRef> {
        match value {
            Value::Object(r) => Some(ObjectRef::from_raw(*r)),
            _ => None,
        }
    }

    pub(crate) fn name_key(&mut self, info: &CodeInfo, name: ConstIndex) -> ObjectKey {
        let name = self.constant_str(info, name);
        self.string_key(&name)
    }

    pub(crate) fn string_key(&mut self, name: &JsStr) -> ObjectKey {
        let atom = self.realm.interner.intern(name);
        self.realm.key(PropertyKey::Atom(atom))
    }

    /// ToPropertyKey, with a fast path for array indices.
    pub(crate) fn object_key(&mut self, key: &Value) -> JsResult<ObjectKey> {
        match key {
            Value::Number(n)
                if n.fract() == 0.0 && *n >= 0.0 && *n <= f64::from(MAX_ARRAY_INDEX) =>
            {
                Ok(ObjectKey::Index(*n as u32))
            }
            Value::String(s) => Ok(self.string_key(s)),
            Value::Symbol(symbol) => Ok(ObjectKey::Symbol(*symbol)),
            other => {
                let key = self.coerce_property_key(other)?;
                self.object_key(&key)
            }
        }
    }

    pub(crate) fn key_to_value(&self, key: ObjectKey) -> Value {
        match key {
            ObjectKey::Index(index) => Value::str(&index.to_string()),
            ObjectKey::Name(atom) => Value::String(self.realm.interner.resolve(atom).clone()),
            ObjectKey::Symbol(symbol) => Value::Symbol(symbol),
        }
    }

    pub(crate) fn key_text(&self, key: ObjectKey) -> String {
        match self.key_to_value(key) {
            Value::String(s) => s.to_string_lossy(),
            _ => "Symbol()".into(),
        }
    }

    /// GetV: property read on any value (primitives read their prototype).
    pub(crate) fn get_value(
        &mut self,
        base: &Value,
        key: ObjectKey,
        site: Option<(&CodeInfo, u32)>,
    ) -> JsResult<Value> {
        if is_private(key) {
            return self.private_get(base, key);
        }
        let object = match base {
            Value::Object(r) => ObjectRef::from_raw(*r),
            Value::Undefined | Value::Null => {
                let message = format!(
                    "Cannot read properties of {} (reading '{}')",
                    self.coerce_string(base)?.to_string_lossy(),
                    self.key_text(key)
                );
                return Err(self.error(ErrorKind::Type, &message));
            }
            Value::String(s) => {
                if let Some(value) = self.string_own_property(s, key) {
                    return Ok(value);
                }
                self.intrinsic(Intrinsic::StringPrototype)
            }
            Value::Number(_) => self.intrinsic(Intrinsic::NumberPrototype),
            Value::Bool(_) => self.intrinsic(Intrinsic::BooleanPrototype),
            Value::Symbol(_) => self.intrinsic(Intrinsic::SymbolPrototype),
            Value::BigInt(_) => self.intrinsic(Intrinsic::BigIntPrototype),
        };
        self.touch(object)?;
        match site {
            Some((info, cache)) if matches!(base, Value::Object(_)) && !self.is_proxy(object) => {
                self.get_cached(info, cache as usize, object, key)
            }
            _ => self.get(object, key, base),
        }
    }

    fn string_own_property(&self, s: &JsStr, key: ObjectKey) -> Option<Value> {
        match key {
            ObjectKey::Index(index) => {
                let unit = *s.to_utf16().get(index as usize)?;
                Some(Value::String(JsStr::from_utf16(vec![unit])))
            }
            key if key == self.length_key() => Some(Value::Number(s.len() as f64)),
            _ => None,
        }
    }

    fn length_key(&self) -> ObjectKey {
        self.state.intrinsics.length_key
    }

    fn get_cached(
        &mut self,
        info: &CodeInfo,
        cache: usize,
        o: ObjectRef,
        key: ObjectKey,
    ) -> JsResult<Value> {
        if !info.caches.borrow()[cache].hits(&self.realm, o) && self.proxy_in_chain(o)? {
            return self.get(o, key, &object_value(o));
        }
        let result = info.caches.borrow_mut()[cache].load(&mut self.realm, o, key);
        match result {
            Err(error) if needs_interpreter(&error) => self.get(o, key, &object_value(o)),
            other => other.map_err(|e| self.object_error(e)),
        }
    }

    /// [[Get]] that can run getters.
    pub(crate) fn get(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        receiver: &Value,
    ) -> JsResult<Value> {
        let mut current = o;
        loop {
            self.touch(current)?;
            match self.exotic(current, key) {
                Exotic::Proxy => return self.proxy_get(current, key, receiver),
                Exotic::Element(array, index) => return Ok(array.read(index)),
                Exotic::Ordinary => {}
            }
            let own = self.realm.get_own_property(current, key);
            match own.map_err(|e| self.object_error(e))? {
                Some(Property {
                    value: PropertyValue::Data { value, .. },
                    ..
                }) => return Ok(value),
                Some(Property {
                    value: PropertyValue::Accessor { get, .. },
                    ..
                }) => {
                    if matches!(get, Value::Undefined) {
                        return Ok(Value::Undefined);
                    }
                    return self.call_value(&get, receiver, &[]);
                }
                None => match self.prototype_of(current)? {
                    Some(proto) => current = proto,
                    None => return Ok(Value::Undefined),
                },
            }
        }
    }

    /// Whether a proxy sits on the prototype chain of `o`; the realm's lookups cannot run its traps.
    fn proxy_in_chain(&mut self, o: ObjectRef) -> JsResult<bool> {
        let mut current = self.prototype_of(o)?;
        while let Some(proto) = current {
            if self.is_proxy(proto) {
                return Ok(true);
            }
            current = self.prototype_of(proto)?;
        }
        Ok(false)
    }

    pub(crate) fn prototype_of(&mut self, o: ObjectRef) -> JsResult<Option<ObjectRef>> {
        let proto = self.realm.get_prototype_of(o);
        proto.map_err(|e| self.object_error(e))
    }

    /// PutValue for `reg.key = acc`; strict code throws when the write fails.
    fn set_value(
        &mut self,
        object: Reg,
        key: ObjectKey,
        site: Option<(&CodeInfo, u32)>,
    ) -> JsResult<()> {
        let base = self.reg(object).clone();
        let value = self.state.acc.clone();
        if is_private(key) {
            self.private_set(&base, key, value.clone())?;
            self.state.acc = value;
            return Ok(());
        }
        let is_strict = self.top().info.code.is_strict;
        let Some(o) = self.as_object(&base) else {
            if matches!(base, Value::Undefined | Value::Null) || is_strict {
                let message = format!(
                    "Cannot set properties of {} (setting '{}')",
                    self.coerce_string(&base)?.to_string_lossy(),
                    self.key_text(key)
                );
                return Err(self.error(ErrorKind::Type, &message));
            }
            return Ok(());
        };
        self.touch(o)?;
        let result = value.clone();
        let done = match site {
            Some((info, cache)) if !self.is_proxy(o) => {
                self.set_cached(info, cache as usize, o, key, value)?
            }
            _ => self.set(o, key, value, &base)?,
        };
        // A setter's return value must not replace the value of the assignment.
        self.state.acc = result;
        if !done && is_strict {
            let message = format!(
                "Cannot assign to read only property '{}'",
                self.key_text(key)
            );
            return Err(self.error(ErrorKind::Type, &message));
        }
        Ok(())
    }

    fn set_cached(
        &mut self,
        info: &CodeInfo,
        cache: usize,
        o: ObjectRef,
        key: ObjectKey,
        value: Value,
    ) -> JsResult<bool> {
        // An object array length needs ToPrimitive, which only the interpreter can run.
        let object_length =
            matches!(value, Value::Object(_)) && key == self.length_key() && self.realm.is_array(o);
        if object_length
            || (!info.caches.borrow()[cache].hits(&self.realm, o) && self.proxy_in_chain(o)?)
        {
            return self.set(o, key, value, &object_value(o));
        }
        let result = info.caches.borrow_mut()[cache].store(&mut self.realm, o, key, value.clone());
        match result {
            Err(error) if needs_interpreter(&error) => self.set(o, key, value, &object_value(o)),
            other => other.map_err(|e| self.object_error(e)),
        }
    }

    /// [[Set]] (OrdinarySet) that can run setters.
    pub(crate) fn set(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        value: Value,
        receiver: &Value,
    ) -> JsResult<bool> {
        let mut current = o;
        loop {
            self.touch(current)?;
            match self.exotic(current, key) {
                Exotic::Proxy => return self.proxy_set(current, key, value, receiver),
                Exotic::Element(array, index) => {
                    if same_value(&object_value(current), receiver) {
                        self.typed_set(&array, index, &value)?;
                        return Ok(true);
                    }
                    if array.valid(index).is_none() {
                        return Ok(true);
                    }
                    // A valid element is a writable data property: OrdinarySet defines it
                    // on the receiver.
                    break;
                }
                Exotic::Ordinary => {}
            }
            let own = self.realm.get_own_property(current, key);
            match own.map_err(|e| self.object_error(e))?.map(|p| p.value) {
                Some(PropertyValue::Data {
                    writable: false, ..
                }) => return Ok(false),
                Some(PropertyValue::Data { .. }) => break,
                Some(PropertyValue::Accessor {
                    set: Value::Undefined,
                    ..
                }) => return Ok(false),
                Some(PropertyValue::Accessor { set, .. }) => {
                    self.call_value(&set, receiver, &[value])?;
                    return Ok(true);
                }
                None => match self.prototype_of(current)? {
                    Some(proto) => current = proto,
                    None => break,
                },
            }
        }
        let Some(receiver) = self.as_object(receiver) else {
            return Ok(false);
        };
        let desc = match self.own_property(receiver, key)?.map(|p| p.value) {
            Some(PropertyValue::Data { writable: true, .. }) => PropertyDescriptor {
                value: Some(value),
                ..PropertyDescriptor::default()
            },
            Some(_) => return Ok(false),
            None => PropertyDescriptor::data(value, true, true, true),
        };
        self.define_own(receiver, key, &desc)
    }

    pub(crate) fn define_property(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        desc: &PropertyDescriptor,
    ) -> JsResult<bool> {
        if key == self.length_key()
            && matches!(desc.value, Some(Value::Object(_)))
            && self.realm.is_array(o)
        {
            return self.define_array_length(o, desc);
        }
        let defined = self.realm.define_own_property(o, key, desc);
        defined.map_err(|e| self.object_error(e))
    }

    /// ArraySetLength steps 3-5 for an object value: ToUint32 and ToNumber run here, where
    /// `valueOf` can be called; the realm then sees a Number.
    fn define_array_length(&mut self, o: ObjectRef, desc: &PropertyDescriptor) -> JsResult<bool> {
        let value = desc.value.clone().unwrap_or(Value::Undefined);
        let length = super::operators::to_uint32(self.coerce_number(&value)?);
        let number = self.coerce_number(&value)?;
        if f64::from(length) != number {
            return Err(self.error(ErrorKind::Range, "Invalid array length"));
        }
        let desc = PropertyDescriptor {
            value: Some(Value::Number(number)),
            ..desc.clone()
        };
        let defined = self.realm.define_own_property(o, self.length_key(), &desc);
        defined.map_err(|e| self.object_error(e))
    }

    /// CreateDataProperty.
    pub(crate) fn create_data_property(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        value: Value,
    ) -> JsResult<bool> {
        self.define_property(o, key, &PropertyDescriptor::data(value, true, true, true))
    }

    fn delete_property(&mut self, base: &Value, key: ObjectKey) -> JsResult<bool> {
        let Some(o) = self.as_object(base) else {
            if matches!(base, Value::Undefined | Value::Null) {
                let message = "Cannot convert undefined or null to object";
                return Err(self.error(ErrorKind::Type, message));
            }
            return Ok(true);
        };
        let deleted = self.delete_key(o, key)?;
        if !deleted && self.top().info.code.is_strict {
            let message = format!("Cannot delete property '{}'", self.key_text(key));
            return Err(self.error(ErrorKind::Type, &message));
        }
        Ok(deleted)
    }

    /// Elements `0..length` of an array-like object.
    pub(crate) fn array_elements(&mut self, array: Value) -> JsResult<Vec<Value>> {
        let Some(o) = self.as_object(&array) else {
            return Err(self.internal("spread arguments are not an array"));
        };
        let length = self.get(o, self.length_key(), &array)?;
        let length = self.coerce_number(&length)? as u32;
        (0..length)
            .map(|index| self.get(o, ObjectKey::Index(index), &array))
            .collect()
    }

    pub(crate) fn object_error(&mut self, error: ObjectError) -> super::Abrupt {
        match error {
            ObjectError::Type(message) => self.error(ErrorKind::Type, &message),
            ObjectError::Range(message) => self.error(ErrorKind::Range, &message),
        }
    }

    pub(crate) fn internal(&mut self, message: &str) -> super::Abrupt {
        self.error(ErrorKind::Error, &format!("internal error: {message}"))
    }
}

//! Abstract operations shared by the built-ins (ECMA-262 §7): type conversions, internal
//! slots, calls, array-likes and constructor helpers.

use super::slots::{Slot, SlotTable};
use super::{Interpreter, Intrinsic, JsResult, object_value};
use crate::bytecode::FunctionKind;
use crate::interpreter::function::{Callable, Closure};
use crate::{ObjectKey, ObjectRef, Value};
use velyr_engine_gc::Gc;

/// 2^53 - 1, the largest length of an array-like.
pub(crate) const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

impl Interpreter {
    fn slot_table(&self) -> Gc<SlotTable> {
        self.state.slots
    }

    pub(crate) fn slot(&self, object: ObjectRef) -> Option<Slot> {
        let table = self.realm.heap.get(self.slot_table())?;
        table.0.get(object.raw()).cloned()
    }

    pub(crate) fn value_slot(&self, value: &Value) -> Option<Slot> {
        self.slot(self.as_object(value)?)
    }

    pub(crate) fn set_slot(&mut self, object: ObjectRef, slot: Slot) {
        let table = self.slot_table();
        let table = self.realm.heap.get_mut(table);
        table
            .expect("slot table is rooted")
            .0
            .insert(object.raw(), slot);
    }

    /// An object with `proto` and internal `slot`, rooted until the native returns.
    pub(crate) fn new_slotted(&mut self, proto: ObjectRef, slot: Slot) -> JsResult<ObjectRef> {
        let object = self.new_object_with(Some(proto))?;
        self.set_slot(object, slot);
        Ok(object)
    }

    /// ToObject: primitives get a wrapper object.
    pub(crate) fn coerce_object(&mut self, value: &Value) -> JsResult<ObjectRef> {
        let (slot, proto) = match value {
            Value::Object(r) => return Ok(ObjectRef::from_raw(*r)),
            Value::Undefined | Value::Null => {
                return self.throw_type("Cannot convert undefined or null to object");
            }
            Value::Bool(b) => (Slot::Boolean(*b), Intrinsic::BooleanPrototype),
            Value::Number(n) => (Slot::Number(*n), Intrinsic::NumberPrototype),
            Value::String(s) => return self.create_string_object(s.clone(), None),
            Value::Symbol(s) => (Slot::Symbol(*s), Intrinsic::SymbolPrototype),
            Value::BigInt(b) => (Slot::BigInt(b.clone()), Intrinsic::BigIntPrototype),
        };
        let proto = self.intrinsic(proto);
        self.new_slotted(proto, slot)
    }

    /// OrdinaryCallBindThis for sloppy functions: a primitive `this` becomes its wrapper.
    /// `f` and `args` stay rooted while the wrapper is allocated.
    pub(crate) fn sloppy_this(
        &mut self,
        closure: &Closure,
        f: &Value,
        this: &Value,
        args: &[Value],
    ) -> JsResult<Value> {
        let code = &closure.info.code;
        let is_lexical = matches!(code.kind, FunctionKind::Arrow | FunctionKind::Script);
        if code.is_strict
            || is_lexical
            || matches!(this, Value::Object(_) | Value::Undefined | Value::Null)
        {
            return Ok(this.clone());
        }
        let mark = self.state.scratch.len();
        self.state.scratch.push(f.clone());
        self.state.scratch.extend_from_slice(args);
        let boxed = self.coerce_object(this);
        self.state.scratch.truncate(mark);
        Ok(object_value(boxed?))
    }

    /// RequireObjectCoercible.
    pub(crate) fn require_coercible(&mut self, value: &Value, method: &str) -> JsResult<()> {
        if matches!(value, Value::Undefined | Value::Null) {
            let message = format!("{method} called on null or undefined");
            return self.throw_type(&message);
        }
        Ok(())
    }

    /// ToIntegerOrInfinity.
    pub(crate) fn coerce_integer(&mut self, value: &Value) -> JsResult<f64> {
        let n = self.coerce_number(value)?;
        Ok(integer_part(n))
    }

    /// ToLength.
    pub(crate) fn coerce_length(&mut self, value: &Value) -> JsResult<f64> {
        let n = self.coerce_integer(value)?;
        Ok(n.clamp(0.0, MAX_SAFE_INTEGER))
    }

    /// LengthOfArrayLike.
    pub(crate) fn length_of(&mut self, object: ObjectRef) -> JsResult<f64> {
        let key = self.state.intrinsics.length_key;
        let length = self.get(object, key, &object_value(object))?;
        self.coerce_length(&length)
    }

    pub(crate) fn is_callable(&self, value: &Value) -> bool {
        self.callable(value).is_some()
    }

    /// Get(O, P) on an object by property name.
    pub(crate) fn get_named(&mut self, object: ObjectRef, name: &str) -> JsResult<Value> {
        let key = self.realm.key_str(name);
        self.get(object, key, &object_value(object))
    }

    /// Get(O, P) with a precomputed key.
    pub(crate) fn get_key(&mut self, object: ObjectRef, key: ObjectKey) -> JsResult<Value> {
        self.get(object, key, &object_value(object))
    }

    /// GetMethod: `None` for `undefined`/`null`, TypeError when not callable.
    pub(crate) fn get_method(&mut self, value: &Value, key: ObjectKey) -> JsResult<Option<Value>> {
        let method = self.get_value(value, key, None)?;
        match method {
            Value::Undefined | Value::Null => Ok(None),
            method if self.is_callable(&method) => Ok(Some(method)),
            _ => {
                let message = format!("{} is not a function", self.key_text(key));
                self.throw_type(&message)
            }
        }
    }

    /// Call(F, V, args), with the callee checked first.
    pub(crate) fn call_function(
        &mut self,
        f: &Value,
        this: &Value,
        args: &[Value],
    ) -> JsResult<Value> {
        if !self.is_callable(f) {
            return Err(self.not_callable(f));
        }
        self.call_value(f, this, args)
    }

    /// Invoke(V, P, args).
    pub(crate) fn invoke(&mut self, value: &Value, name: &str, args: &[Value]) -> JsResult<Value> {
        let key = self.realm.key_str(name);
        let f = self.get_value(value, key, None)?;
        self.call_function(&f, value, args)
    }

    /// CreateArrayFromList; the result is rooted until the native returns.
    pub(crate) fn array_from_list(&mut self, values: &[Value]) -> JsResult<Value> {
        let array = object_value(self.create_array_from(values)?);
        self.keep(&array);
        Ok(array)
    }

    /// CreateListFromArrayLike.
    pub(crate) fn list_from_array_like(&mut self, value: &Value) -> JsResult<Vec<Value>> {
        let Some(object) = self.as_object(value) else {
            return self.throw_type("CreateListFromArrayLike called on non-object");
        };
        let length = self.length_of(object)?;
        let mut list = Vec::new();
        let mut index = 0.0;
        while index < length {
            let key = self.index_key(index);
            let element = self.get(object, key, value)?;
            self.keep(&element);
            list.push(element);
            index += 1.0;
        }
        Ok(list)
    }

    /// The property key of a non-negative integer (an array index or its string form).
    pub(crate) fn index_key(&mut self, index: f64) -> ObjectKey {
        if index <= f64::from(crate::MAX_ARRAY_INDEX) {
            return ObjectKey::Index(index as u32);
        }
        let text = crate::number_to_string(index);
        self.realm.key_str(&text)
    }

    /// GetPrototypeFromConstructor(constructor, intrinsicDefaultProto).
    pub(crate) fn proto_from_constructor(
        &mut self,
        constructor: &Value,
        default: ObjectRef,
    ) -> JsResult<ObjectRef> {
        let Some(object) = self.as_object(constructor) else {
            return Ok(default);
        };
        let proto = self.get_named(object, "prototype")?;
        match self.as_object(&proto) {
            Some(proto) => Ok(proto),
            None => self.function_realm(object).map(|_| default),
        }
    }

    /// GetFunctionRealm, which only has one realm to find: it throws for a revoked proxy
    /// reached through proxy targets and bound function targets.
    pub(crate) fn function_realm(&mut self, object: ObjectRef) -> JsResult<()> {
        if self.is_proxy(object) {
            let (target, _) = self.proxy_parts(object)?;
            return self.function_realm(target);
        }
        if let Some(Callable::Bound(bound)) = self.callable(&object_value(object))
            && let Some(target) = self.as_object(&bound.target)
        {
            return self.function_realm(target);
        }
        Ok(())
    }

    /// OrdinaryCreateFromConstructor for the running native constructor: the prototype
    /// comes from `NewTarget` (when called with `new`), else `default`.
    pub(crate) fn create_from_new_target(
        &mut self,
        new_target: &Value,
        default: ObjectRef,
        slot: Option<Slot>,
    ) -> JsResult<ObjectRef> {
        let proto = self.proto_from_constructor(new_target, default)?;
        let object = self.new_object_with(Some(proto))?;
        if let Some(slot) = slot {
            self.set_slot(object, slot);
        }
        Ok(object)
    }

    /// `this` as an object for a `Foo.prototype.method` that needs one.
    pub(crate) fn this_object_for(&mut self, this: &Value, method: &str) -> JsResult<ObjectRef> {
        match self.as_object(this) {
            Some(object) => Ok(object),
            None => {
                let message = format!("{method} called on a non-object");
                self.throw_type(&message)
            }
        }
    }

    /// ToString of a value as a Rust `String` (lossy for lone surrogates).
    pub(crate) fn coerce_rust_string(&mut self, value: &Value) -> JsResult<String> {
        Ok(self.coerce_string(value)?.to_string_lossy())
    }
}

/// The mathematical integer part, with NaN as 0 (ToIntegerOrInfinity on a Number).
pub(crate) fn integer_part(n: f64) -> f64 {
    if n.is_nan() {
        return 0.0;
    }
    let truncated = n.trunc();
    if truncated == 0.0 { 0.0 } else { truncated }
}

/// A relative index argument (`-1` is the last) clamped into `0..=length`.
pub(crate) fn relative_index(relative: f64, length: f64) -> f64 {
    if relative < 0.0 {
        (length + relative).max(0.0)
    } else {
        relative.min(length)
    }
}

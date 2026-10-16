//! The `String` constructor, its statics and String exotic objects (ECMA-262 §22.1).
//! The prototype methods live in the submodules.

mod access;
mod html;
mod pattern;
mod transform;

use super::slots::Slot;
use super::symbol::SYMBOL_ITERATOR;
use super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::interpreter::operators::to_uint32;
use crate::{JsStr, ObjectKey, ObjectRef, PropertyDescriptor, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::StringPrototype);
    vm.set_slot(proto, Slot::String(JsStr::from_str("")));
    let length = vm.state.intrinsics.length_key;
    let desc = PropertyDescriptor::data(Value::Number(0.0), false, false, false);
    vm.define_property(proto, length, &desc)?;
    let constructor = vm.constructor("String", 1, string_constructor, proto)?;
    vm.method(constructor, "fromCharCode", 1, string_from_char_code)?;
    vm.method(constructor, "fromCodePoint", 1, string_from_code_point)?;
    vm.method(constructor, "raw", 1, string_raw)?;
    vm.symbol_method(proto, SYMBOL_ITERATOR, 0, string_iterator)?;
    access::install(vm, proto)?;
    transform::install(vm, proto)?;
    pattern::install(vm, proto)?;
    html::install(vm, proto)
}

/// Code units taken by the code point at `position` (2 for a surrogate pair).
pub(crate) fn code_point_size(units: &[u16], position: usize) -> usize {
    let lead = units[position];
    let is_pair = (0xD800..0xDC00).contains(&lead)
        && units
            .get(position + 1)
            .is_some_and(|t| (0xDC00..0xE000).contains(t));
    if is_pair { 2 } else { 1 }
}

/// A string value from UTF-16 code units.
pub(crate) fn string_value(units: Vec<u16>) -> Value {
    Value::String(JsStr::from_utf16(units))
}

/// `RequireObjectCoercible(this)` then `ToString`, as code units.
pub(crate) fn this_units(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<Vec<u16>> {
    vm.require_coercible(this, &format!("String.prototype.{method}"))?;
    Ok(vm.coerce_string(this)?.to_utf16())
}

impl Interpreter {
    /// StringCreate: a String exotic object for `text`.
    /// shortcut: the index properties are materialized as real own properties (O(length)
    /// at creation); switch to a virtual [[GetOwnProperty]] if large wrappers matter.
    pub(crate) fn create_string_object(
        &mut self,
        text: JsStr,
        proto: Option<ObjectRef>,
    ) -> JsResult<ObjectRef> {
        let proto = proto.unwrap_or_else(|| self.intrinsic(Intrinsic::StringPrototype));
        let object = self.new_slotted(proto, Slot::String(text.clone()))?;
        let units = text.to_utf16();
        for (index, unit) in units.iter().enumerate() {
            let value = string_value(vec![*unit]);
            let desc = PropertyDescriptor::data(value, false, true, false);
            self.define_property(object, ObjectKey::Index(index as u32), &desc)?;
        }
        let length = self.state.intrinsics.length_key;
        let desc = PropertyDescriptor::data(Value::Number(units.len() as f64), false, false, false);
        self.define_property(object, length, &desc)?;
        Ok(object)
    }

    /// thisStringValue.
    pub(crate) fn this_string_value(&mut self, this: &Value, method: &str) -> JsResult<JsStr> {
        match this {
            Value::String(s) => Ok(s.clone()),
            _ => match self.value_slot(this) {
                Some(Slot::String(s)) => Ok(s),
                _ => self.throw_type(&format!("{method} requires that 'this' be a String")),
            },
        }
    }
}

fn string_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    let text = match args.first() {
        None => JsStr::from_str(""),
        Some(Value::Symbol(symbol)) if matches!(new_target, Value::Undefined) => {
            return Ok(Value::String(vm.symbol_descriptive_string(*symbol)));
        }
        Some(value) => vm.coerce_string(value)?,
    };
    if matches!(new_target, Value::Undefined) {
        return Ok(Value::String(text));
    }
    let default = vm.intrinsic(Intrinsic::StringPrototype);
    let proto = vm.proto_from_constructor(&new_target, default)?;
    Ok(object_value(vm.create_string_object(text, Some(proto))?))
}

fn string_from_char_code(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let mut units = Vec::with_capacity(args.len());
    for value in args {
        let n = vm.coerce_number(value)?;
        units.push(to_uint32(n) as u16);
    }
    Ok(string_value(units))
}

fn string_from_code_point(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let mut units = Vec::with_capacity(args.len());
    for value in args {
        let n = vm.coerce_number(value)?;
        if n.trunc() != n || !(0.0..=1_114_111.0).contains(&n) {
            let text = crate::number_to_string(n);
            return vm.throw_range(&format!("Invalid code point {text}"));
        }
        push_code_point(&mut units, n as u32);
    }
    Ok(string_value(units))
}

/// UTF16EncodeCodePoint (lone surrogates stay single units).
pub(crate) fn push_code_point(units: &mut Vec<u16>, code_point: u32) {
    if code_point < 0x10000 {
        units.push(code_point as u16);
    } else {
        let c = code_point - 0x10000;
        units.push(0xD800 + (c >> 10) as u16);
        units.push(0xDC00 + (c & 0x3FF) as u16);
    }
}

fn string_raw(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let cooked = vm.coerce_object(&arg(args, 0))?;
    let raw = vm.get_named(cooked, "raw")?;
    let raw = vm.coerce_object(&raw)?;
    let literal_count = vm.length_of(raw)?;
    let mut out = Vec::new();
    let mut index = 0.0;
    while index < literal_count {
        let key = vm.index_key(index);
        let segment = vm.get_key(raw, key)?;
        out.extend(vm.coerce_string(&segment)?.to_utf16());
        if index + 1.0 >= literal_count {
            break;
        }
        if let Some(substitution) = args.get(index as usize + 1) {
            out.extend(vm.coerce_string(substitution)?.to_utf16());
        }
        index += 1.0;
    }
    Ok(string_value(out))
}

fn string_iterator(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    vm.require_coercible(this, "String.prototype[Symbol.iterator]")?;
    let text = vm.coerce_string(this)?;
    vm.create_string_iterator(text)
}

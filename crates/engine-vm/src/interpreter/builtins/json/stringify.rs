//! `JSON.stringify` (ECMA-262 §25.5.2): replacer function or property list, gap,
//! `toJSON`, wrapper unboxing and cycle detection.

use super::super::slots::Slot;
use super::super::{Interpreter, JsResult, arg, object_value};
use super::enumerable_keys;
use crate::{JsStr, ObjectKey, ObjectRef, Value};

struct State {
    replacer: Option<Value>,
    property_list: Option<Vec<ObjectKey>>,
    stack: Vec<ObjectRef>,
    indent: Vec<u16>,
    gap: Vec<u16>,
}

pub(super) fn json_stringify(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let replacer = arg(args, 1);
    let mut state = State {
        replacer: None,
        property_list: None,
        stack: Vec::new(),
        indent: Vec::new(),
        gap: Vec::new(),
    };
    if let Some(object) = vm.as_object(&replacer) {
        if vm.is_callable(&replacer) {
            state.replacer = Some(replacer.clone());
        } else if vm.is_array(&replacer)? {
            state.property_list = Some(property_list(vm, object)?);
        }
    }
    state.gap = gap(vm, &arg(args, 2))?;
    let wrapper = vm.new_object()?;
    let empty = vm.string_key(&JsStr::from_str(""));
    vm.create_data_or_throw(wrapper, empty, arg(args, 0))?;
    match serialize_property(vm, &mut state, empty, wrapper)? {
        Some(units) => Ok(Value::String(JsStr::from_utf16(units))),
        None => Ok(Value::Undefined),
    }
}

fn property_list(vm: &mut Interpreter, replacer: ObjectRef) -> JsResult<Vec<ObjectKey>> {
    let length = vm.length_of(replacer)?;
    let mut list = Vec::new();
    let mut index = 0.0;
    while index < length {
        let key = vm.index_key(index);
        let value = vm.get_key(replacer, key)?;
        let item = match &value {
            Value::String(s) => Some(s.clone()),
            Value::Number(_) => Some(vm.coerce_string(&value)?),
            Value::Object(_) => match vm.value_slot(&value) {
                Some(Slot::String(_) | Slot::Number(_)) => Some(vm.coerce_string(&value)?),
                _ => None,
            },
            _ => None,
        };
        if let Some(item) = item {
            let key = vm.string_key(&item);
            if !list.contains(&key) {
                list.push(key);
            }
        }
        index += 1.0;
    }
    Ok(list)
}

fn gap(vm: &mut Interpreter, space: &Value) -> JsResult<Vec<u16>> {
    let space = match vm.value_slot(space) {
        Some(Slot::Number(_)) => Value::Number(vm.coerce_number(space)?),
        Some(Slot::String(_)) => Value::String(vm.coerce_string(space)?),
        _ => space.clone(),
    };
    Ok(match space {
        Value::Number(n) => {
            let count = super::super::abstract_ops::integer_part(n).clamp(0.0, 10.0);
            vec![u16::from(b' '); count as usize]
        }
        Value::String(s) => s.to_utf16().into_iter().take(10).collect(),
        _ => Vec::new(),
    })
}

/// SerializeJSONProperty; `None` is `undefined` (the property is skipped).
fn serialize_property(
    vm: &mut Interpreter,
    state: &mut State,
    key: ObjectKey,
    holder: ObjectRef,
) -> JsResult<Option<Vec<u16>>> {
    let mut value = vm.get_key(holder, key)?;
    if matches!(value, Value::Object(_) | Value::BigInt(_)) {
        let to_json_key = vm.realm.key_str("toJSON");
        let to_json = vm.get_value(&value, to_json_key, None)?;
        if vm.is_callable(&to_json) {
            let name = vm.key_to_value(key);
            value = vm.call_value(&to_json, &value, &[name])?;
        }
    }
    if let Some(replacer) = state.replacer.clone() {
        vm.keep(&value);
        let name = vm.key_to_value(key);
        value = vm.call_value(&replacer, &object_value(holder), &[name, value])?;
    }
    vm.keep(&value);
    value = match vm.value_slot(&value) {
        Some(Slot::Number(_)) => Value::Number(vm.coerce_number(&value)?),
        Some(Slot::String(_)) => Value::String(vm.coerce_string(&value)?),
        Some(Slot::Boolean(b)) => Value::Bool(b),
        Some(Slot::BigInt(b)) => Value::BigInt(b),
        _ => value,
    };
    let text = match &value {
        Value::Null => "null".encode_utf16().collect(),
        Value::Bool(true) => "true".encode_utf16().collect(),
        Value::Bool(false) => "false".encode_utf16().collect(),
        Value::String(s) => quote(&s.to_utf16()),
        Value::Number(n) if n.is_finite() => crate::number_to_string(*n).encode_utf16().collect(),
        Value::Number(_) => "null".encode_utf16().collect(),
        Value::BigInt(_) => return vm.throw_type("Do not know how to serialize a BigInt"),
        Value::Object(_) if matches!(vm.value_slot(&value), Some(Slot::RawJson)) => {
            let object = vm.as_object(&value).expect("checked object");
            let key = vm.realm.key_str("rawJSON");
            let raw = vm.get_key(object, key)?;
            vm.coerce_string(&raw)?.to_utf16()
        }
        Value::Object(_) if !vm.is_callable(&value) => {
            let object = vm.as_object(&value).expect("checked object");
            match vm.is_array(&value)? {
                true => serialize_array(vm, state, object)?,
                false => serialize_object(vm, state, object)?,
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(text))
}

/// QuoteJSONString.
fn quote(units: &[u16]) -> Vec<u16> {
    let mut out = vec![u16::from(b'"')];
    let mut index = 0;
    while index < units.len() {
        let unit = units[index];
        let escape = match unit {
            0x08 => Some("\\b".to_string()),
            0x09 => Some("\\t".to_string()),
            0x0A => Some("\\n".to_string()),
            0x0C => Some("\\f".to_string()),
            0x0D => Some("\\r".to_string()),
            0x22 => Some("\\\"".to_string()),
            0x5C => Some("\\\\".to_string()),
            0..0x20 => Some(format!("\\u{unit:04x}")),
            0xD800..0xDC00
                if units
                    .get(index + 1)
                    .is_some_and(|t| (0xDC00..0xE000).contains(t)) =>
            {
                out.push(unit);
                out.push(units[index + 1]);
                index += 2;
                continue;
            }
            0xD800..0xE000 => Some(format!("\\u{unit:04x}")),
            _ => None,
        };
        match escape {
            Some(text) => out.extend(text.encode_utf16()),
            None => out.push(unit),
        }
        index += 1;
    }
    out.push(u16::from(b'"'));
    out
}

fn enter(vm: &mut Interpreter, state: &mut State, object: ObjectRef) -> JsResult<Vec<u16>> {
    if state.stack.contains(&object) {
        return vm.throw_type("Converting circular structure to JSON");
    }
    state.stack.push(object);
    let stepback = state.indent.clone();
    state.indent.extend_from_slice(&state.gap.clone());
    Ok(stepback)
}

/// Joins serialized members with the gap layout and leaves the nesting level.
fn finish(
    state: &mut State,
    stepback: Vec<u16>,
    parts: Vec<Vec<u16>>,
    open: u8,
    close: u8,
) -> Vec<u16> {
    let mut out = vec![u16::from(open)];
    if !parts.is_empty() {
        let separator: Vec<u16> = match state.gap.is_empty() {
            true => vec![u16::from(b',')],
            false => [&[u16::from(b','), u16::from(b'\n')][..], &state.indent].concat(),
        };
        if !state.gap.is_empty() {
            out.push(u16::from(b'\n'));
            out.extend_from_slice(&state.indent);
        }
        out.extend(parts.join(&separator[..]));
        if !state.gap.is_empty() {
            out.push(u16::from(b'\n'));
            out.extend_from_slice(&stepback);
        }
    }
    out.push(u16::from(close));
    state.stack.pop();
    state.indent = stepback;
    out
}

fn serialize_object(
    vm: &mut Interpreter,
    state: &mut State,
    object: ObjectRef,
) -> JsResult<Vec<u16>> {
    let stepback = enter(vm, state, object)?;
    let keys = match &state.property_list {
        Some(list) => list.clone(),
        None => enumerable_keys(vm, object)?,
    };
    let mut parts = Vec::new();
    for key in keys {
        if let Some(text) = serialize_property(vm, state, key, object)? {
            let name = vm.key_to_value(key);
            let name = vm.coerce_string(&name)?.to_utf16();
            let mut member = quote(&name);
            member.push(u16::from(b':'));
            if !state.gap.is_empty() {
                member.push(u16::from(b' '));
            }
            member.extend(text);
            parts.push(member);
        }
    }
    Ok(finish(state, stepback, parts, b'{', b'}'))
}

fn serialize_array(
    vm: &mut Interpreter,
    state: &mut State,
    object: ObjectRef,
) -> JsResult<Vec<u16>> {
    let stepback = enter(vm, state, object)?;
    let length = vm.length_of(object)?;
    let mut parts = Vec::new();
    let mut index = 0.0;
    while index < length {
        let key = vm.index_key(index);
        let text = serialize_property(vm, state, key, object)?;
        parts.push(text.unwrap_or_else(|| "null".encode_utf16().collect()));
        index += 1.0;
    }
    Ok(finish(state, stepback, parts, b'[', b']'))
}

//! `Array.prototype` methods that read elements without callbacks: `at`, `concat`,
//! `includes`, `indexOf`, `lastIndexOf`, `join`, `toString`, `toLocaleString`, `slice`,
//! `with` and the iterator factories.

use super::super::abstract_ops::relative_index;
use super::super::object::prototype::object_to_string;
use super::super::slots::IterationKind;
use super::super::symbol::SYMBOL_IS_CONCAT_SPREADABLE;
use super::super::{Interpreter, JsResult, arg, object_value};
use super::array_iterator;
use crate::{JsStr, ObjectKey, ObjectRef, Value, same_value_zero, strict_equals, to_boolean};

pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    vm.method(proto, "at", 1, array_at)?;
    vm.method(proto, "concat", 1, array_concat)?;
    vm.method(proto, "entries", 0, array_entries)?;
    vm.method(proto, "includes", 1, array_includes)?;
    vm.method(proto, "indexOf", 1, array_index_of)?;
    vm.method(proto, "join", 1, array_join)?;
    vm.method(proto, "keys", 0, array_keys)?;
    vm.method(proto, "lastIndexOf", 1, array_last_index_of)?;
    vm.method(proto, "slice", 2, array_slice)?;
    vm.method(proto, "toLocaleString", 0, array_to_locale_string)?;
    vm.method(proto, "toString", 0, array_to_string)?;
    vm.method(proto, "values", 0, array_values)?;
    vm.method(proto, "with", 2, array_with)
}

/// `ToObject(this)` and its length.
pub(super) fn this_array(vm: &mut Interpreter, this: &Value) -> JsResult<(ObjectRef, f64)> {
    let object = vm.coerce_object(this)?;
    let length = vm.length_of(object)?;
    Ok((object, length))
}

/// A relative start/end argument; `undefined` becomes `default`.
pub(super) fn relative_arg(
    vm: &mut Interpreter,
    value: &Value,
    length: f64,
    default: f64,
) -> JsResult<f64> {
    if matches!(value, Value::Undefined) {
        return Ok(default);
    }
    let relative = vm.coerce_integer(value)?;
    Ok(relative_index(relative, length))
}

pub(in super::super) fn array_at(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let relative = vm.coerce_integer(&arg(args, 0))?;
    let index = if relative >= 0.0 {
        relative
    } else {
        length + relative
    };
    if index < 0.0 || index >= length {
        return Ok(Value::Undefined);
    }
    let key = vm.index_key(index);
    vm.get_key(object, key)
}

fn is_concat_spreadable(vm: &mut Interpreter, value: &Value) -> JsResult<bool> {
    let Some(object) = vm.as_object(value) else {
        return Ok(false);
    };
    let spreadable = vm.get_key(object, ObjectKey::Symbol(SYMBOL_IS_CONCAT_SPREADABLE))?;
    if !matches!(spreadable, Value::Undefined) {
        return Ok(to_boolean(&spreadable));
    }
    vm.is_array(value)
}

fn array_concat(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let object = vm.coerce_object(this)?;
    let result = vm.array_species_create(object, 0.0)?;
    let mut n = 0.0;
    let items = std::iter::once(object_value(object)).chain(args.iter().cloned());
    for item in items {
        if !is_concat_spreadable(vm, &item)? {
            vm.check_length(n + 1.0)?;
            let key = vm.index_key(n);
            vm.create_data_or_throw(result, key, item)?;
            n += 1.0;
            continue;
        }
        let source = vm.as_object(&item).expect("spreadable values are objects");
        let length = vm.length_of(source)?;
        vm.check_length(n + length)?;
        let mut k = 0.0;
        while k < length {
            let key = vm.index_key(k);
            if vm.has_property(source, key)? {
                let value = vm.get_key(source, key)?;
                let target = vm.index_key(n);
                vm.create_data_or_throw(result, target, value)?;
            }
            n += 1.0;
            k += 1.0;
        }
    }
    vm.set_length(result, n)?;
    Ok(object_value(result))
}

pub(in super::super) fn array_includes(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    if length == 0.0 {
        return Ok(Value::Bool(false));
    }
    let mut k = relative_arg(vm, &arg(args, 1), length, 0.0)?;
    let target = arg(args, 0);
    while k < length {
        let key = vm.index_key(k);
        let element = vm.get_key(object, key)?;
        if same_value_zero(&element, &target) {
            return Ok(Value::Bool(true));
        }
        k += 1.0;
    }
    Ok(Value::Bool(false))
}

pub(in super::super) fn array_index_of(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    if length == 0.0 {
        return Ok(Value::Number(-1.0));
    }
    let mut k = relative_arg(vm, &arg(args, 1), length, 0.0)?;
    let target = arg(args, 0);
    while k < length {
        let key = vm.index_key(k);
        if vm.has_property(object, key)? {
            let element = vm.get_key(object, key)?;
            if strict_equals(&element, &target) {
                return Ok(Value::Number(k));
            }
        }
        k += 1.0;
    }
    Ok(Value::Number(-1.0))
}

pub(in super::super) fn array_last_index_of(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    if length == 0.0 {
        return Ok(Value::Number(-1.0));
    }
    let mut k = match args.get(1) {
        None => length - 1.0,
        Some(from) => {
            let from = vm.coerce_integer(from)?;
            if from >= 0.0 {
                from.min(length - 1.0)
            } else {
                length + from
            }
        }
    };
    let target = arg(args, 0);
    while k >= 0.0 {
        let key = vm.index_key(k);
        if vm.has_property(object, key)? {
            let element = vm.get_key(object, key)?;
            if strict_equals(&element, &target) {
                return Ok(Value::Number(k));
            }
        }
        k -= 1.0;
    }
    Ok(Value::Number(-1.0))
}

pub(in super::super) fn array_join(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let separator = match arg(args, 0) {
        Value::Undefined => JsStr::from_str(","),
        value => vm.coerce_string(&value)?,
    };
    let separator = separator.to_utf16();
    let mut out: Vec<u16> = Vec::new();
    let mut k = 0.0;
    while k < length {
        if k > 0.0 {
            out.extend_from_slice(&separator);
        }
        let key = vm.index_key(k);
        let element = vm.get_key(object, key)?;
        if !matches!(element, Value::Undefined | Value::Null) {
            out.extend(vm.coerce_string(&element)?.to_utf16());
        }
        k += 1.0;
    }
    Ok(Value::String(JsStr::from_utf16(out)))
}

fn array_to_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let object = vm.coerce_object(this)?;
    let join = vm.get_named(object, "join")?;
    if vm.is_callable(&join) {
        return vm.call_value(&join, &object_value(object), &[]);
    }
    object_to_string(vm, &object_value(object), &[])
}

pub(in super::super) fn array_to_locale_string(
    vm: &mut Interpreter,
    this: &Value,
    _: &[Value],
) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let mut out: Vec<u16> = Vec::new();
    let mut k = 0.0;
    while k < length {
        if k > 0.0 {
            out.push(u16::from(b','));
        }
        let key = vm.index_key(k);
        let element = vm.get_key(object, key)?;
        if !matches!(element, Value::Undefined | Value::Null) {
            let text = vm.invoke(&element, "toLocaleString", &[])?;
            out.extend(vm.coerce_string(&text)?.to_utf16());
        }
        k += 1.0;
    }
    Ok(Value::String(JsStr::from_utf16(out)))
}

fn array_slice(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let mut k = relative_arg(vm, &arg(args, 0), length, 0.0)?;
    let end = relative_arg(vm, &arg(args, 1), length, length)?;
    let count = (end - k).max(0.0);
    let result = vm.array_species_create(object, count)?;
    let mut n = 0.0;
    while k < end {
        let key = vm.index_key(k);
        if vm.has_property(object, key)? {
            let value = vm.get_key(object, key)?;
            let target = vm.index_key(n);
            vm.create_data_or_throw(result, target, value)?;
        }
        k += 1.0;
        n += 1.0;
    }
    vm.set_length(result, n)?;
    Ok(object_value(result))
}

fn array_with(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let relative = vm.coerce_integer(&arg(args, 0))?;
    let actual = if relative >= 0.0 {
        relative
    } else {
        length + relative
    };
    if actual >= length || actual < 0.0 {
        return vm.throw_range("Array.prototype.with: index out of range");
    }
    let result = vm.array_create(length, None)?;
    let replacement = arg(args, 1);
    let mut k = 0.0;
    while k < length {
        let key = vm.index_key(k);
        let value = match k == actual {
            true => replacement.clone(),
            false => vm.get_key(object, key)?,
        };
        vm.create_data_or_throw(result, key, value)?;
        k += 1.0;
    }
    Ok(object_value(result))
}

pub(in super::super) fn array_keys(
    vm: &mut Interpreter,
    this: &Value,
    _: &[Value],
) -> JsResult<Value> {
    array_iterator(vm, this, IterationKind::Keys)
}

pub(in super::super) fn array_values(
    vm: &mut Interpreter,
    this: &Value,
    _: &[Value],
) -> JsResult<Value> {
    array_iterator(vm, this, IterationKind::Values)
}

pub(in super::super) fn array_entries(
    vm: &mut Interpreter,
    this: &Value,
    _: &[Value],
) -> JsResult<Value> {
    array_iterator(vm, this, IterationKind::Entries)
}

//! `%TypedArray%.prototype` (ECMA-262 §23.2.3). Methods whose generic Array algorithm
//! gives the same result reuse it after ValidateTypedArray; the ones that create typed
//! arrays, convert elements once or sort numerically are written here.

use std::cmp::Ordering;
use std::rc::Rc;

use super::super::abstract_ops::relative_index;
use super::super::array::{access, callbacks, mutators, reorder};
use super::super::symbol::{SYMBOL_ITERATOR, SYMBOL_TO_STRING_TAG};
use super::super::{Builtin, Interpreter, JsResult, arg, object_value};
use super::constructors::{copy_elements, create_from_constructor, species_create};
use super::{ElementKind, TypedArray};
use crate::{ObjectKey, ObjectRef, PropertyDescriptor, Value, to_boolean};

macro_rules! delegated {
    ($($name:ident => $target:path, $method:literal;)*) => {
        $(fn $name(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
            vm.validate_typed_array(this, $method)?;
            $target(vm, this, args)
        })*
    };
}

delegated! {
    at => access::array_at, "%TypedArray%.prototype.at";
    copy_within => mutators::array_copy_within, "%TypedArray%.prototype.copyWithin";
    entries => access::array_entries, "%TypedArray%.prototype.entries";
    every => callbacks::array_every, "%TypedArray%.prototype.every";
    find => callbacks::array_find, "%TypedArray%.prototype.find";
    find_index => callbacks::array_find_index, "%TypedArray%.prototype.findIndex";
    find_last => callbacks::array_find_last, "%TypedArray%.prototype.findLast";
    find_last_index => callbacks::array_find_last_index, "%TypedArray%.prototype.findLastIndex";
    for_each => callbacks::array_for_each, "%TypedArray%.prototype.forEach";
    includes => access::array_includes, "%TypedArray%.prototype.includes";
    index_of => access::array_index_of, "%TypedArray%.prototype.indexOf";
    join => access::array_join, "%TypedArray%.prototype.join";
    keys => access::array_keys, "%TypedArray%.prototype.keys";
    last_index_of => access::array_last_index_of, "%TypedArray%.prototype.lastIndexOf";
    reduce => callbacks::array_reduce, "%TypedArray%.prototype.reduce";
    reduce_right => callbacks::array_reduce_right, "%TypedArray%.prototype.reduceRight";
    reverse => reorder::array_reverse, "%TypedArray%.prototype.reverse";
    some => callbacks::array_some, "%TypedArray%.prototype.some";
    to_locale_string => access::array_to_locale_string, "%TypedArray%.prototype.toLocaleString";
    values => access::array_values, "%TypedArray%.prototype.values";
}

const METHODS: [(&str, u32, Builtin); 30] = [
    ("at", 1, at),
    ("copyWithin", 2, copy_within),
    ("entries", 0, entries),
    ("every", 1, every),
    ("fill", 1, fill),
    ("filter", 1, filter),
    ("find", 1, find),
    ("findIndex", 1, find_index),
    ("findLast", 1, find_last),
    ("findLastIndex", 1, find_last_index),
    ("forEach", 1, for_each),
    ("includes", 1, includes),
    ("indexOf", 1, index_of),
    ("join", 1, join),
    ("keys", 0, keys),
    ("lastIndexOf", 1, last_index_of),
    ("map", 1, map),
    ("reduce", 1, reduce),
    ("reduceRight", 1, reduce_right),
    ("reverse", 0, reverse),
    ("set", 1, set),
    ("slice", 2, slice),
    ("some", 1, some),
    ("sort", 1, sort),
    ("subarray", 2, subarray),
    ("toLocaleString", 0, to_locale_string),
    ("toReversed", 0, to_reversed),
    ("toSorted", 1, to_sorted),
    ("values", 0, values),
    ("with", 2, with),
];

pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    vm.getter(proto, "buffer", buffer)?;
    vm.getter(proto, "byteLength", byte_length)?;
    vm.getter(proto, "byteOffset", byte_offset)?;
    vm.getter(proto, "length", length)?;
    let tag = ObjectKey::Symbol(SYMBOL_TO_STRING_TAG);
    vm.accessor(proto, tag, "[Symbol.toStringTag]", to_string_tag, None)?;
    for (name, arity, f) in METHODS {
        vm.method(proto, name, arity, f)?;
    }
    let array_prototype = vm.state.intrinsics.array_prototype;
    let to_string = vm.get_named(array_prototype, "toString")?;
    vm.define_method(proto, "toString", to_string)?;
    let values = vm.get_named(proto, "values")?;
    let desc = PropertyDescriptor::data(values, true, false, true);
    vm.define_property(proto, ObjectKey::Symbol(SYMBOL_ITERATOR), &desc)?;
    Ok(())
}

fn this_typed_array(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<Rc<TypedArray>> {
    match vm.typed_array_of(this) {
        Some(array) => Ok(array),
        None => vm.throw_type(&format!("{method}: this is not a typed array")),
    }
}

fn buffer(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let array = this_typed_array(vm, this, "get %TypedArray%.prototype.buffer")?;
    Ok(object_value(array.view.buffer))
}

fn byte_length(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let array = this_typed_array(vm, this, "get %TypedArray%.prototype.byteLength")?;
    let length = array.length().unwrap_or(0) * array.kind.size();
    Ok(Value::Number(length as f64))
}

fn byte_offset(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let array = this_typed_array(vm, this, "get %TypedArray%.prototype.byteOffset")?;
    let offset = array.length().map_or(0, |_| array.view.byte_offset);
    Ok(Value::Number(offset as f64))
}

fn length(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let array = this_typed_array(vm, this, "get %TypedArray%.prototype.length")?;
    Ok(Value::Number(array.length().unwrap_or(0) as f64))
}

fn to_string_tag(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(match vm.typed_array_of(this) {
        Some(array) => Value::str(array.kind.name()),
        None => Value::Undefined,
    })
}

/// A relative index argument; `undefined` becomes `default`.
fn relative(vm: &mut Interpreter, value: &Value, length: usize, default: usize) -> JsResult<usize> {
    if matches!(value, Value::Undefined) {
        return Ok(default);
    }
    let relative = vm.coerce_integer(value)?;
    Ok(relative_index(relative, length as f64) as usize)
}

/// The current length, or a TypeError once a conversion detached or shrank the buffer.
fn revalidate(vm: &mut Interpreter, array: &TypedArray, method: &str) -> JsResult<usize> {
    match array.length() {
        Some(length) => Ok(length),
        None => vm.throw_type(&format!(
            "{method}: the typed array is detached or out of bounds"
        )),
    }
}

fn fill(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let method = "%TypedArray%.prototype.fill";
    let (array, length) = vm.validate_typed_array(this, method)?;
    let value = vm.element_value(array.kind, &arg(args, 0))?;
    let start = relative(vm, &arg(args, 1), length, 0)?;
    let end = relative(vm, &arg(args, 2), length, length)?;
    let end = end.min(revalidate(vm, &array, method)?);
    let raw = array.kind.encode(&value);
    for index in start..end {
        array.store_raw(index, raw);
    }
    Ok(this.clone())
}

fn callback_arg(vm: &mut Interpreter, args: &[Value], method: &str) -> JsResult<Value> {
    let callback = arg(args, 0);
    if !vm.is_callable(&callback) {
        return vm.throw_type(&format!("{method}: callback is not a function"));
    }
    Ok(callback)
}

fn map(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let method = "%TypedArray%.prototype.map";
    let (array, length) = vm.validate_typed_array(this, method)?;
    let callback = callback_arg(vm, args, method)?;
    let this_arg = arg(args, 1);
    let length_value = Value::Number(length as f64);
    let (target, _) = species_create(vm, this, array.kind, &[length_value])?;
    for index in 0..length {
        let element = array.read(Some(index));
        let call_args = [element, Value::Number(index as f64), this.clone()];
        let mapped = vm.call_value(&callback, &this_arg, &call_args)?;
        let key = vm.index_key(index as f64);
        vm.set_or_throw(target, key, mapped)?;
    }
    Ok(object_value(target))
}

fn filter(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let method = "%TypedArray%.prototype.filter";
    let (array, length) = vm.validate_typed_array(this, method)?;
    let callback = callback_arg(vm, args, method)?;
    let this_arg = arg(args, 1);
    let mut kept = Vec::new();
    for index in 0..length {
        let element = array.read(Some(index));
        let call_args = [element.clone(), Value::Number(index as f64), this.clone()];
        if to_boolean(&vm.call_value(&callback, &this_arg, &call_args)?) {
            kept.push(element);
        }
    }
    let count = Value::Number(kept.len() as f64);
    let (target, _) = species_create(vm, this, array.kind, &[count])?;
    for (index, element) in kept.into_iter().enumerate() {
        let key = vm.index_key(index as f64);
        vm.set_or_throw(target, key, element)?;
    }
    Ok(object_value(target))
}

fn set(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let method = "%TypedArray%.prototype.set";
    let target = this_typed_array(vm, this, method)?;
    let offset = vm.coerce_integer(&arg(args, 1))?;
    if offset < 0.0 {
        return vm.throw_range(&format!("{method}: offset is out of bounds"));
    }
    let source = arg(args, 0);
    let target_length = revalidate(vm, &target, method)?;
    if let Some(source) = vm.typed_array_of(&source) {
        let source_length = revalidate(vm, &source, method)?;
        if source.kind.is_bigint() != target.kind.is_bigint() {
            return vm.throw_type("Cannot mix BigInt and other types, use explicit conversions");
        }
        if offset + source_length as f64 > target_length as f64 {
            return vm.throw_range(&format!("{method}: offset is out of bounds"));
        }
        copy_elements(&source, &target, offset as usize, source_length);
        return Ok(Value::Undefined);
    }
    let source = vm.coerce_object(&source)?;
    let source_length = vm.length_of(source)?;
    if offset + source_length > target_length as f64 {
        return vm.throw_range(&format!("{method}: offset is out of bounds"));
    }
    for index in 0..source_length as usize {
        let key = vm.index_key(index as f64);
        let value = vm.get_key(source, key)?;
        vm.typed_set(&target, Some(offset as usize + index), &value)?;
    }
    Ok(Value::Undefined)
}

fn slice(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let method = "%TypedArray%.prototype.slice";
    let (array, length) = vm.validate_typed_array(this, method)?;
    let start = relative(vm, &arg(args, 0), length, 0)?;
    let end = relative(vm, &arg(args, 1), length, length)?;
    let count = end.saturating_sub(start);
    let (target_object, target) =
        species_create(vm, this, array.kind, &[Value::Number(count as f64)])?;
    if count > 0 {
        let end = end.min(revalidate(vm, &array, method)?);
        for index in start..end {
            let raw = array.load_raw(index);
            let raw = match array.kind == target.kind {
                true => raw,
                false => target.kind.encode(&array.kind.decode(raw)),
            };
            target.store_raw(index - start, raw);
        }
    }
    Ok(object_value(target_object))
}

fn subarray(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let array = this_typed_array(vm, this, "%TypedArray%.prototype.subarray")?;
    let length = array.length().unwrap_or(0);
    let start = relative(vm, &arg(args, 0), length, 0)?;
    let byte_offset = array.view.byte_offset + start * array.kind.size();
    let mut new_args = vec![
        object_value(array.view.buffer),
        Value::Number(byte_offset as f64),
    ];
    let end = arg(args, 1);
    if array.view.fixed.is_some() || !matches!(end, Value::Undefined) {
        let end = relative(vm, &end, length, length)?;
        new_args.push(Value::Number(end.saturating_sub(start) as f64));
    }
    let (target, _) = species_create(vm, this, array.kind, &new_args)?;
    Ok(object_value(target))
}

/// The default TypedArray order: numeric, `-0` before `+0`, NaN last.
fn default_order(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.is_nan(), y.is_nan()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => x
                .partial_cmp(y)
                .unwrap_or(Ordering::Equal)
                .then_with(|| y.is_sign_negative().cmp(&x.is_sign_negative())),
        },
        (Value::BigInt(x), Value::BigInt(y)) => crate::bigint_ops::compare(x, y),
        _ => Ordering::Equal,
    }
}

fn comparator_arg(vm: &mut Interpreter, args: &[Value], method: &str) -> JsResult<Value> {
    let compare = arg(args, 0);
    if !matches!(compare, Value::Undefined) && !vm.is_callable(&compare) {
        return vm.throw_type(&format!(
            "{method}: the comparison function must be a function"
        ));
    }
    Ok(compare)
}

/// The elements in sorted order (a stable merge sort, as SortIndexedProperties requires).
fn sorted_elements(
    vm: &mut Interpreter,
    array: &TypedArray,
    length: usize,
    compare: &Value,
) -> JsResult<Vec<Value>> {
    let mut values: Vec<Value> = (0..length).map(|index| array.read(Some(index))).collect();
    if matches!(compare, Value::Undefined) {
        values.sort_by(default_order);
        return Ok(values);
    }
    let mut failure = None;
    values.sort_by(|a, b| {
        if failure.is_some() {
            return Ordering::Equal;
        }
        let order = vm
            .call_value(compare, &Value::Undefined, &[a.clone(), b.clone()])
            .and_then(|result| vm.coerce_number(&result));
        match order {
            Ok(n) if n < 0.0 => Ordering::Less,
            Ok(n) if n > 0.0 => Ordering::Greater,
            Ok(_) => Ordering::Equal,
            Err(error) => {
                failure = Some(error);
                Ordering::Equal
            }
        }
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(values),
    }
}

/// Writes `values` from index 0, skipping indices a comparator made invalid.
fn write_elements(array: &TypedArray, values: &[Value]) {
    for (index, value) in values.iter().enumerate() {
        if array.valid(Some(index)).is_some() {
            array.store_raw(index, array.kind.encode(value));
        }
    }
}

fn sort(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let method = "%TypedArray%.prototype.sort";
    let compare = comparator_arg(vm, args, method)?;
    let (array, length) = vm.validate_typed_array(this, method)?;
    let values = sorted_elements(vm, &array, length, &compare)?;
    write_elements(&array, &values);
    Ok(this.clone())
}

/// TypedArrayCreateSameType.
fn same_type(
    vm: &mut Interpreter,
    kind: ElementKind,
    length: usize,
) -> JsResult<(ObjectRef, Rc<TypedArray>)> {
    let constructor = object_value(vm.intrinsic(kind.constructor()));
    create_from_constructor(vm, &constructor, &[Value::Number(length as f64)])
}

fn to_sorted(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let method = "%TypedArray%.prototype.toSorted";
    let compare = comparator_arg(vm, args, method)?;
    let (array, length) = vm.validate_typed_array(this, method)?;
    let (target_object, target) = same_type(vm, array.kind, length)?;
    let values = sorted_elements(vm, &array, length, &compare)?;
    write_elements(&target, &values);
    Ok(object_value(target_object))
}

fn to_reversed(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let (array, length) = vm.validate_typed_array(this, "%TypedArray%.prototype.toReversed")?;
    let (target_object, target) = same_type(vm, array.kind, length)?;
    for index in 0..length {
        target.store_raw(index, array.load_raw(length - 1 - index));
    }
    Ok(object_value(target_object))
}

fn with(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let method = "%TypedArray%.prototype.with";
    let (array, length) = vm.validate_typed_array(this, method)?;
    let relative = vm.coerce_integer(&arg(args, 0))?;
    let actual = if relative >= 0.0 {
        relative
    } else {
        length as f64 + relative
    };
    let value = vm.element_value(array.kind, &arg(args, 1))?;
    let index = (actual >= 0.0).then_some(actual as usize);
    if array.valid(index).is_none() {
        return vm.throw_range(&format!("{method}: index is out of bounds"));
    }
    let (target_object, target) = same_type(vm, array.kind, length)?;
    let replaced = array.kind.encode(&value);
    for position in 0..length {
        let raw = match Some(position) == index {
            true => replaced,
            false => array.load_raw(position),
        };
        target.store_raw(position, raw);
    }
    Ok(object_value(target_object))
}

//! `Array.prototype` methods that reorder elements: `reverse`, `toReversed`, `sort` and
//! `toSorted`.

use std::cmp::Ordering;

use super::super::{Interpreter, JsResult, arg, object_value};
use super::access::this_array;
use crate::{ObjectRef, Value};

pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    vm.method(proto, "reverse", 0, array_reverse)?;
    vm.method(proto, "sort", 1, array_sort)?;
    vm.method(proto, "toReversed", 0, array_to_reversed)?;
    vm.method(proto, "toSorted", 1, array_to_sorted)
}

pub(in super::super) fn array_reverse(
    vm: &mut Interpreter,
    this: &Value,
    _: &[Value],
) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let middle = (length / 2.0).floor();
    let mut lower = 0.0;
    while lower != middle {
        let upper = length - lower - 1.0;
        let lower_key = vm.index_key(lower);
        let upper_key = vm.index_key(upper);
        let lower_exists = vm.has_property(object, lower_key)?;
        let lower_value = match lower_exists {
            true => vm.get_key(object, lower_key)?,
            false => Value::Undefined,
        };
        vm.keep(&lower_value);
        let upper_exists = vm.has_property(object, upper_key)?;
        let upper_value = match upper_exists {
            true => vm.get_key(object, upper_key)?,
            false => Value::Undefined,
        };
        vm.keep(&upper_value);
        match (lower_exists, upper_exists) {
            (true, true) => {
                vm.set_or_throw(object, lower_key, upper_value)?;
                vm.set_or_throw(object, upper_key, lower_value)?;
            }
            (false, true) => {
                vm.set_or_throw(object, lower_key, upper_value)?;
                vm.delete_or_throw(object, upper_key)?;
            }
            (true, false) => {
                vm.delete_or_throw(object, lower_key)?;
                vm.set_or_throw(object, upper_key, lower_value)?;
            }
            (false, false) => {}
        }
        lower += 1.0;
    }
    Ok(object_value(object))
}

fn array_to_reversed(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let result = vm.array_create(length, None)?;
    let mut k = 0.0;
    while k < length {
        let from = vm.index_key(length - k - 1.0);
        let value = vm.get_key(object, from)?;
        let to = vm.index_key(k);
        vm.create_data_or_throw(result, to, value)?;
        k += 1.0;
    }
    Ok(object_value(result))
}

/// The `comparefn` argument: `undefined` or callable.
fn comparator_arg(vm: &mut Interpreter, args: &[Value], method: &str) -> JsResult<Option<Value>> {
    match arg(args, 0) {
        Value::Undefined => Ok(None),
        f if vm.is_callable(&f) => Ok(Some(f)),
        _ => vm.throw_type(&format!(
            "Array.prototype.{method}: comparator must be a function"
        )),
    }
}

/// SortCompare: `undefined` sorts last; then `comparefn`, or code-unit string order.
fn sort_compare(
    vm: &mut Interpreter,
    compare: &Option<Value>,
    x: &Value,
    y: &Value,
) -> JsResult<Ordering> {
    match (x, y) {
        (Value::Undefined, Value::Undefined) => return Ok(Ordering::Equal),
        (Value::Undefined, _) => return Ok(Ordering::Greater),
        (_, Value::Undefined) => return Ok(Ordering::Less),
        _ => {}
    }
    if let Some(compare) = compare {
        let result = vm.call_value(compare, &Value::Undefined, &[x.clone(), y.clone()])?;
        let n = vm.coerce_number(&result)?;
        return Ok(n.partial_cmp(&0.0).unwrap_or(Ordering::Equal));
    }
    let x = vm.coerce_string(x)?.to_utf16();
    let y = vm.coerce_string(y)?.to_utf16();
    Ok(x.cmp(&y))
}

/// Stable merge sort with a fallible comparator.
fn merge_sort(
    vm: &mut Interpreter,
    items: Vec<Value>,
    compare: &Option<Value>,
) -> JsResult<Vec<Value>> {
    if items.len() <= 1 {
        return Ok(items);
    }
    let mut right = items;
    let left = right.drain(..right.len() / 2).collect();
    let left = merge_sort(vm, left, compare)?;
    let right = merge_sort(vm, right, compare)?;
    let mut merged = Vec::with_capacity(left.len() + right.len());
    let mut left = left.into_iter().peekable();
    let mut right = right.into_iter().peekable();
    while let (Some(l), Some(r)) = (left.peek(), right.peek()) {
        if sort_compare(vm, compare, l, r)? == Ordering::Greater {
            merged.extend(right.next());
        } else {
            merged.extend(left.next());
        }
    }
    merged.extend(left);
    merged.extend(right);
    Ok(merged)
}

/// SortIndexedProperties: reads `0..length` (skipping holes when `skip_holes`), roots
/// the values and sorts them.
fn sorted_values(
    vm: &mut Interpreter,
    object: ObjectRef,
    length: f64,
    skip_holes: bool,
    compare: &Option<Value>,
) -> JsResult<Vec<Value>> {
    let mut items = Vec::new();
    let mut k = 0.0;
    while k < length {
        let key = vm.index_key(k);
        if !skip_holes || vm.has_property(object, key)? {
            let value = vm.get_key(object, key)?;
            vm.keep(&value);
            items.push(value);
        }
        k += 1.0;
    }
    merge_sort(vm, items, compare)
}

fn array_sort(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let compare = comparator_arg(vm, args, "sort")?;
    let (object, length) = this_array(vm, this)?;
    let sorted = sorted_values(vm, object, length, true, &compare)?;
    let count = sorted.len() as f64;
    for (index, value) in sorted.into_iter().enumerate() {
        let key = vm.index_key(index as f64);
        vm.set_or_throw(object, key, value)?;
    }
    let mut k = count;
    while k < length {
        let key = vm.index_key(k);
        vm.delete_or_throw(object, key)?;
        k += 1.0;
    }
    Ok(object_value(object))
}

fn array_to_sorted(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let compare = comparator_arg(vm, args, "toSorted")?;
    let (object, length) = this_array(vm, this)?;
    let result = vm.array_create(length, None)?;
    let sorted = sorted_values(vm, object, length, false, &compare)?;
    for (index, value) in sorted.into_iter().enumerate() {
        let key = vm.index_key(index as f64);
        vm.create_data_or_throw(result, key, value)?;
    }
    Ok(object_value(result))
}

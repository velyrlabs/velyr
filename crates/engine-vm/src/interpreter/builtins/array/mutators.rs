//! `Array.prototype` methods that add, remove or overwrite elements: `push`, `pop`,
//! `shift`, `unshift`, `splice`, `toSpliced`, `fill` and `copyWithin`.

use super::super::{Interpreter, JsResult, arg, object_value};
use super::access::{relative_arg, this_array};
use crate::{ObjectRef, Value};

pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    vm.method(proto, "copyWithin", 2, array_copy_within)?;
    vm.method(proto, "fill", 1, array_fill)?;
    vm.method(proto, "pop", 0, array_pop)?;
    vm.method(proto, "push", 1, array_push)?;
    vm.method(proto, "shift", 0, array_shift)?;
    vm.method(proto, "splice", 2, array_splice)?;
    vm.method(proto, "toSpliced", 2, array_to_spliced)?;
    vm.method(proto, "unshift", 1, array_unshift)
}

/// Moves `from` to `to` (or deletes `to` when `from` is a hole).
fn move_element(vm: &mut Interpreter, object: ObjectRef, from: f64, to: f64) -> JsResult<()> {
    let from_key = vm.index_key(from);
    let to_key = vm.index_key(to);
    if vm.has_property(object, from_key)? {
        let value = vm.get_key(object, from_key)?;
        vm.set_or_throw(object, to_key, value)
    } else {
        vm.delete_or_throw(object, to_key)
    }
}

fn array_push(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let count = args.len() as f64;
    vm.check_length(length + count)?;
    for (offset, value) in args.iter().enumerate() {
        let key = vm.index_key(length + offset as f64);
        vm.set_or_throw(object, key, value.clone())?;
    }
    vm.set_length(object, length + count)?;
    Ok(Value::Number(length + count))
}

fn array_pop(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    if length == 0.0 {
        vm.set_length(object, 0.0)?;
        return Ok(Value::Undefined);
    }
    let key = vm.index_key(length - 1.0);
    let element = vm.get_key(object, key)?;
    vm.keep(&element);
    vm.delete_or_throw(object, key)?;
    vm.set_length(object, length - 1.0)?;
    Ok(element)
}

fn array_shift(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    if length == 0.0 {
        vm.set_length(object, 0.0)?;
        return Ok(Value::Undefined);
    }
    let first_key = vm.index_key(0.0);
    let first = vm.get_key(object, first_key)?;
    vm.keep(&first);
    let mut k = 1.0;
    while k < length {
        move_element(vm, object, k, k - 1.0)?;
        k += 1.0;
    }
    let last = vm.index_key(length - 1.0);
    vm.delete_or_throw(object, last)?;
    vm.set_length(object, length - 1.0)?;
    Ok(first)
}

fn array_unshift(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let count = args.len() as f64;
    if count > 0.0 {
        vm.check_length(length + count)?;
        let mut k = length;
        while k > 0.0 {
            move_element(vm, object, k - 1.0, k + count - 1.0)?;
            k -= 1.0;
        }
        for (index, value) in args.iter().enumerate() {
            let key = vm.index_key(index as f64);
            vm.set_or_throw(object, key, value.clone())?;
        }
    }
    vm.set_length(object, length + count)?;
    Ok(Value::Number(length + count))
}

/// `(actualStart, actualSkipCount)` of `splice`/`toSpliced`.
fn splice_bounds(vm: &mut Interpreter, args: &[Value], length: f64) -> JsResult<(f64, f64)> {
    let start = relative_arg(vm, &arg(args, 0), length, 0.0)?;
    let skip = match args.len() {
        0 => 0.0,
        1 => length - start,
        _ => vm.coerce_integer(&args[1])?.clamp(0.0, length - start),
    };
    Ok((start, skip))
}

fn array_splice(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let (start, delete_count) = splice_bounds(vm, args, length)?;
    let items = args.get(2..).unwrap_or(&[]);
    let item_count = items.len() as f64;
    vm.check_length(length + item_count - delete_count)?;
    let removed = vm.array_species_create(object, delete_count)?;
    vm.rooted(&[object_value(object), object_value(removed)], |vm| {
        let mut since_check = 0;
        let mut k = 0.0;
        while k < delete_count {
            let from = vm.index_key(start + k);
            if vm.has_property(object, from)? {
                let value = vm.get_key(object, from)?;
                let to = vm.index_key(k);
                vm.create_data_or_throw(removed, to, value)?;
            }
            k += 1.0;
            vm.heap_tick(&mut since_check, 0)?;
        }
        Ok(())
    })?;
    vm.set_length(removed, delete_count)?;
    if item_count < delete_count {
        let mut k = start;
        while k < length - delete_count {
            move_element(vm, object, k + delete_count, k + item_count)?;
            k += 1.0;
        }
        let mut k = length;
        while k > length - delete_count + item_count {
            let key = vm.index_key(k - 1.0);
            vm.delete_or_throw(object, key)?;
            k -= 1.0;
        }
    } else if item_count > delete_count {
        let mut k = length - delete_count;
        while k > start {
            move_element(vm, object, k + delete_count - 1.0, k + item_count - 1.0)?;
            k -= 1.0;
        }
    }
    for (offset, item) in items.iter().enumerate() {
        let key = vm.index_key(start + offset as f64);
        vm.set_or_throw(object, key, item.clone())?;
    }
    vm.set_length(object, length - delete_count + item_count)?;
    Ok(object_value(removed))
}

fn array_to_spliced(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let (start, skip) = splice_bounds(vm, args, length)?;
    let items = args.get(2..).unwrap_or(&[]);
    let new_length = length + items.len() as f64 - skip;
    vm.check_length(new_length)?;
    let result = vm.array_create(new_length, None)?;
    let mut i = 0.0;
    while i < start {
        let key = vm.index_key(i);
        let value = vm.get_key(object, key)?;
        vm.create_data_or_throw(result, key, value)?;
        i += 1.0;
    }
    for item in items {
        let key = vm.index_key(i);
        vm.create_data_or_throw(result, key, item.clone())?;
        i += 1.0;
    }
    let mut from = start + skip;
    while i < new_length {
        let from_key = vm.index_key(from);
        let value = vm.get_key(object, from_key)?;
        let key = vm.index_key(i);
        vm.create_data_or_throw(result, key, value)?;
        i += 1.0;
        from += 1.0;
    }
    Ok(object_value(result))
}

fn array_fill(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let value = arg(args, 0);
    let mut k = relative_arg(vm, &arg(args, 1), length, 0.0)?;
    let end = relative_arg(vm, &arg(args, 2), length, length)?;
    // Element storage is charged as it grows but checked only at safepoints, so a long
    // fill checks the heap limit itself; `object` is rooted for the collections it runs.
    vm.rooted(&[object_value(object)], |vm| {
        let mut since_check = 0;
        while k < end {
            let key = vm.index_key(k);
            vm.set_or_throw(object, key, value.clone())?;
            k += 1.0;
            vm.heap_tick(&mut since_check, 0)?;
        }
        Ok(object_value(object))
    })
}

pub(in super::super) fn array_copy_within(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let mut to = relative_arg(vm, &arg(args, 0), length, 0.0)?;
    let mut from = relative_arg(vm, &arg(args, 1), length, 0.0)?;
    let end = relative_arg(vm, &arg(args, 2), length, length)?;
    let mut count = (end - from).min(length - to);
    let step = if from < to && to < from + count {
        from += count - 1.0;
        to += count - 1.0;
        -1.0
    } else {
        1.0
    };
    vm.rooted(&[object_value(object)], |vm| {
        let mut since_check = 0;
        while count > 0.0 {
            move_element(vm, object, from, to)?;
            from += step;
            to += step;
            count -= 1.0;
            vm.heap_tick(&mut since_check, 0)?;
        }
        Ok(object_value(object))
    })
}

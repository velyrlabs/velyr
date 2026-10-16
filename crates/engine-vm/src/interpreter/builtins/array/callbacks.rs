//! `Array.prototype` methods driven by a callback: `every`, `some`, `forEach`, `map`,
//! `filter`, the `find*` family, `reduce`, `reduceRight`, `flat` and `flatMap`.

use super::super::{Interpreter, JsResult, arg, object_value};
use super::access::this_array;
use crate::{ObjectRef, Value, to_boolean};

pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    vm.method(proto, "every", 1, array_every)?;
    vm.method(proto, "filter", 1, array_filter)?;
    vm.method(proto, "find", 1, array_find)?;
    vm.method(proto, "findIndex", 1, array_find_index)?;
    vm.method(proto, "findLast", 1, array_find_last)?;
    vm.method(proto, "findLastIndex", 1, array_find_last_index)?;
    vm.method(proto, "flat", 0, array_flat)?;
    vm.method(proto, "flatMap", 1, array_flat_map)?;
    vm.method(proto, "forEach", 1, array_for_each)?;
    vm.method(proto, "map", 1, array_map)?;
    vm.method(proto, "reduce", 1, array_reduce)?;
    vm.method(proto, "reduceRight", 1, array_reduce_right)?;
    vm.method(proto, "some", 1, array_some)
}

/// What a per-element visitor asks the loop to do next.
enum Step {
    Continue,
    Stop(Value),
}

/// Visits present elements `0..length` in order (holes skipped), calling
/// `callback(element, index, object)` and handing its result to `visit`.
fn for_each_present(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    method: &str,
    mut visit: impl FnMut(&mut Interpreter, ObjectRef, f64, Value, Value) -> JsResult<Step>,
) -> JsResult<Option<Value>> {
    let (object, length) = this_array(vm, this)?;
    let callback = vm.callback_arg(args, method)?;
    let this_arg = arg(args, 1);
    let mut k = 0.0;
    while k < length {
        let key = vm.index_key(k);
        if vm.has_property(object, key)? {
            let element = vm.get_key(object, key)?;
            vm.keep(&element);
            let call_args = [element.clone(), Value::Number(k), object_value(object)];
            let result = vm.call_value(&callback, &this_arg, &call_args)?;
            if let Step::Stop(value) = visit(vm, object, k, element, result)? {
                return Ok(Some(value));
            }
        }
        k += 1.0;
    }
    Ok(None)
}

pub(in super::super) fn array_every(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let stopped = for_each_present(vm, this, args, "every", |_, _, _, _, result| {
        Ok(match to_boolean(&result) {
            true => Step::Continue,
            false => Step::Stop(Value::Bool(false)),
        })
    })?;
    Ok(stopped.unwrap_or(Value::Bool(true)))
}

pub(in super::super) fn array_some(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let stopped = for_each_present(vm, this, args, "some", |_, _, _, _, result| {
        Ok(match to_boolean(&result) {
            true => Step::Stop(Value::Bool(true)),
            false => Step::Continue,
        })
    })?;
    Ok(stopped.unwrap_or(Value::Bool(false)))
}

pub(in super::super) fn array_for_each(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    for_each_present(vm, this, args, "forEach", |_, _, _, _, _| {
        Ok(Step::Continue)
    })?;
    Ok(Value::Undefined)
}

fn array_map(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let callback = vm.callback_arg(args, "map")?;
    let this_arg = arg(args, 1);
    let result = vm.array_species_create(object, length)?;
    let mut k = 0.0;
    while k < length {
        let key = vm.index_key(k);
        if vm.has_property(object, key)? {
            let element = vm.get_key(object, key)?;
            let call_args = [element, Value::Number(k), object_value(object)];
            let mapped = vm.call_value(&callback, &this_arg, &call_args)?;
            vm.create_data_or_throw(result, key, mapped)?;
        }
        k += 1.0;
    }
    Ok(object_value(result))
}

fn array_filter(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let callback = vm.callback_arg(args, "filter")?;
    let this_arg = arg(args, 1);
    let result = vm.array_species_create(object, 0.0)?;
    let mut to = 0.0;
    let mut k = 0.0;
    while k < length {
        let key = vm.index_key(k);
        if vm.has_property(object, key)? {
            let element = vm.get_key(object, key)?;
            vm.keep(&element);
            let call_args = [element.clone(), Value::Number(k), object_value(object)];
            let selected = vm.call_value(&callback, &this_arg, &call_args)?;
            if to_boolean(&selected) {
                let target = vm.index_key(to);
                vm.create_data_or_throw(result, target, element)?;
                to += 1.0;
            }
        }
        k += 1.0;
    }
    Ok(object_value(result))
}

/// FindViaPredicate: visits every index (holes included) in the given direction and
/// returns `(index, element)` of the first match.
fn find_via_predicate(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    method: &str,
    ascending: bool,
) -> JsResult<Option<(f64, Value)>> {
    let (object, length) = this_array(vm, this)?;
    let predicate = vm.callback_arg(args, method)?;
    let this_arg = arg(args, 1);
    let mut step = 0.0;
    while step < length {
        let k = if ascending { step } else { length - 1.0 - step };
        let key = vm.index_key(k);
        let element = vm.get_key(object, key)?;
        vm.keep(&element);
        let call_args = [element.clone(), Value::Number(k), object_value(object)];
        let found = vm.call_value(&predicate, &this_arg, &call_args)?;
        if to_boolean(&found) {
            return Ok(Some((k, element)));
        }
        step += 1.0;
    }
    Ok(None)
}

pub(in super::super) fn array_find(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let found = find_via_predicate(vm, this, args, "find", true)?;
    Ok(found.map_or(Value::Undefined, |(_, value)| value))
}

pub(in super::super) fn array_find_index(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let found = find_via_predicate(vm, this, args, "findIndex", true)?;
    Ok(Value::Number(found.map_or(-1.0, |(index, _)| index)))
}

pub(in super::super) fn array_find_last(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let found = find_via_predicate(vm, this, args, "findLast", false)?;
    Ok(found.map_or(Value::Undefined, |(_, value)| value))
}

pub(in super::super) fn array_find_last_index(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let found = find_via_predicate(vm, this, args, "findLastIndex", false)?;
    Ok(Value::Number(found.map_or(-1.0, |(index, _)| index)))
}

fn reduce(vm: &mut Interpreter, this: &Value, args: &[Value], ascending: bool) -> JsResult<Value> {
    let method = if ascending { "reduce" } else { "reduceRight" };
    let (object, length) = this_array(vm, this)?;
    let callback = vm.callback_arg(args, method)?;
    let index_at = |step: f64| if ascending { step } else { length - 1.0 - step };
    let mut step = 0.0;
    let mut accumulator = match args.get(1) {
        Some(initial) => initial.clone(),
        None => loop {
            if step >= length {
                return vm.throw_type("Reduce of empty array with no initial value");
            }
            let key = vm.index_key(index_at(step));
            step += 1.0;
            if vm.has_property(object, key)? {
                break vm.get_key(object, key)?;
            }
        },
    };
    while step < length {
        let k = index_at(step);
        let key = vm.index_key(k);
        if vm.has_property(object, key)? {
            vm.keep(&accumulator);
            let element = vm.get_key(object, key)?;
            let call_args = [accumulator, element, Value::Number(k), object_value(object)];
            accumulator = vm.call_value(&callback, &Value::Undefined, &call_args)?;
        }
        step += 1.0;
    }
    Ok(accumulator)
}

pub(in super::super) fn array_reduce(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    reduce(vm, this, args, true)
}

pub(in super::super) fn array_reduce_right(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    reduce(vm, this, args, false)
}

/// FlattenIntoArray; returns the next target index.
#[allow(clippy::too_many_arguments)]
fn flatten_into(
    vm: &mut Interpreter,
    target: ObjectRef,
    source: ObjectRef,
    source_length: f64,
    start: f64,
    depth: f64,
    mapper: Option<(&Value, &Value)>,
) -> JsResult<f64> {
    let mut target_index = start;
    let mut k = 0.0;
    while k < source_length {
        let key = vm.index_key(k);
        if vm.has_property(source, key)? {
            let mut element = vm.get_key(source, key)?;
            if let Some((mapper, this_arg)) = mapper {
                let call_args = [element, Value::Number(k), object_value(source)];
                element = vm.call_value(mapper, this_arg, &call_args)?;
            }
            vm.keep(&element);
            if depth > 0.0 && vm.is_array(&element)? {
                let inner = vm.as_object(&element).expect("arrays are objects");
                let inner_length = vm.length_of(inner)?;
                target_index = flatten_into(
                    vm,
                    target,
                    inner,
                    inner_length,
                    target_index,
                    depth - 1.0,
                    None,
                )?;
            } else {
                vm.check_length(target_index + 1.0)?;
                let target_key = vm.index_key(target_index);
                vm.create_data_or_throw(target, target_key, element)?;
                target_index += 1.0;
            }
        }
        k += 1.0;
    }
    Ok(target_index)
}

fn array_flat(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let depth = match arg(args, 0) {
        Value::Undefined => 1.0,
        value => vm.coerce_integer(&value)?.max(0.0),
    };
    let result = vm.array_species_create(object, 0.0)?;
    flatten_into(vm, result, object, length, 0.0, depth, None)?;
    Ok(object_value(result))
}

fn array_flat_map(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (object, length) = this_array(vm, this)?;
    let mapper = vm.callback_arg(args, "flatMap")?;
    let this_arg = arg(args, 1);
    let result = vm.array_species_create(object, 0.0)?;
    flatten_into(
        vm,
        result,
        object,
        length,
        0.0,
        1.0,
        Some((&mapper, &this_arg)),
    )?;
    Ok(object_value(result))
}

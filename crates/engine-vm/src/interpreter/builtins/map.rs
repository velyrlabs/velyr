//! The `Map` constructor and prototype (ECMA-262 §24.1).

use std::cell::RefCell;
use std::rc::Rc;

use super::array::species_getter;
use super::collections::OrderedTable;
use super::slots::{IterationKind, Slot};
use super::symbol::{SYMBOL_ITERATOR, SYMBOL_SPECIES};
use super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::{ObjectKey, PropertyDescriptor, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::MapPrototype);
    let constructor = vm.constructor("Map", 0, map_constructor, proto)?;
    vm.method(constructor, "groupBy", 2, map_group_by)?;
    let species = ObjectKey::Symbol(SYMBOL_SPECIES);
    vm.accessor(
        constructor,
        species,
        "[Symbol.species]",
        species_getter,
        None,
    )?;
    vm.method(proto, "clear", 0, map_clear)?;
    vm.method(proto, "delete", 1, map_delete)?;
    vm.method(proto, "entries", 0, map_entries)?;
    vm.method(proto, "forEach", 1, map_for_each)?;
    vm.method(proto, "get", 1, map_get)?;
    vm.method(proto, "has", 1, map_has)?;
    vm.method(proto, "keys", 0, map_keys)?;
    vm.method(proto, "set", 2, map_set)?;
    let size = vm.realm.key_str("size");
    vm.accessor(proto, size, "size", map_size, None)?;
    vm.method(proto, "values", 0, map_values)?;
    let entries = vm.get_named(proto, "entries")?;
    let desc = PropertyDescriptor::data(entries, true, false, true);
    vm.define_property(proto, ObjectKey::Symbol(SYMBOL_ITERATOR), &desc)?;
    vm.define_to_string_tag(proto, "Map")
}

/// The `[[MapData]]` of `this`.
fn this_map(
    vm: &mut Interpreter,
    this: &Value,
    method: &str,
) -> JsResult<Rc<RefCell<OrderedTable>>> {
    match vm.value_slot(this) {
        Some(Slot::Map(table)) => Ok(table),
        _ => vm.throw_type(&format!(
            "Method Map.prototype.{method} called on incompatible receiver"
        )),
    }
}

fn map_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        return vm.throw_type("Constructor Map requires 'new'");
    }
    let proto = vm.intrinsic(Intrinsic::MapPrototype);
    let table = Rc::new(RefCell::new(OrderedTable::default()));
    let map = vm.create_from_new_target(&new_target, proto, Some(Slot::Map(table)))?;
    let iterable = arg(args, 0);
    if matches!(iterable, Value::Undefined | Value::Null) {
        return Ok(object_value(map));
    }
    let adder = vm.get_named(map, "set")?;
    if !vm.is_callable(&adder) {
        return vm.throw_type("Map.prototype.set is not a function");
    }
    add_entries(vm, &object_value(map), &iterable, &adder)?;
    Ok(object_value(map))
}

/// AddEntriesFromIterable.
pub(crate) fn add_entries(
    vm: &mut Interpreter,
    target: &Value,
    iterable: &Value,
    adder: &Value,
) -> JsResult<()> {
    vm.iterate(iterable, |vm, item| {
        let Some(entry) = vm.as_object(&item) else {
            return vm.throw_type("Iterator value is not an entry object");
        };
        let key = vm.get_key(entry, ObjectKey::Index(0))?;
        vm.keep(&key);
        let value = vm.get_key(entry, ObjectKey::Index(1))?;
        vm.call_value(adder, target, &[key, value])?;
        Ok(())
    })
}

fn map_group_by(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let groups = vm.group_by(&arg(args, 0), &arg(args, 1), false)?;
    let mut table = OrderedTable::default();
    for (key, values) in groups {
        let array = vm.array_from_list(&values)?;
        table.set(key, array);
    }
    let proto = vm.intrinsic(Intrinsic::MapPrototype);
    let map = vm.new_slotted(proto, Slot::Map(Rc::new(RefCell::new(table))))?;
    Ok(object_value(map))
}

fn map_clear(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    this_map(vm, this, "clear")?.borrow_mut().clear();
    Ok(Value::Undefined)
}

fn map_delete(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let table = this_map(vm, this, "delete")?;
    let deleted = table.borrow_mut().delete(&arg(args, 0));
    Ok(Value::Bool(deleted))
}

fn map_get(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let table = this_map(vm, this, "get")?;
    let value = table.borrow().get(&arg(args, 0)).cloned();
    Ok(value.unwrap_or(Value::Undefined))
}

fn map_has(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let table = this_map(vm, this, "has")?;
    let has = table.borrow().has(&arg(args, 0));
    Ok(Value::Bool(has))
}

fn map_set(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let table = this_map(vm, this, "set")?;
    table.borrow_mut().set(arg(args, 0), arg(args, 1));
    Ok(this.clone())
}

fn map_size(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let table = this_map(vm, this, "size")?;
    let size = table.borrow().size();
    Ok(Value::Number(size as f64))
}

/// Visits live entries in order, including ones added during the walk.
pub(crate) fn for_each_entry(
    vm: &mut Interpreter,
    table: &Rc<RefCell<OrderedTable>>,
    mut f: impl FnMut(&mut Interpreter, Value, Value) -> JsResult<()>,
) -> JsResult<()> {
    let mut position = 0;
    loop {
        let entry = table.borrow().entry_from(position);
        let Some((next, key, value)) = entry else {
            return Ok(());
        };
        position = next;
        vm.keep(&key);
        vm.keep(&value);
        f(vm, key, value)?;
    }
}

fn map_for_each(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let table = this_map(vm, this, "forEach")?;
    let callback = arg(args, 0);
    if !vm.is_callable(&callback) {
        return vm.throw_type("Map.prototype.forEach: callback is not a function");
    }
    let this_arg = arg(args, 1);
    for_each_entry(vm, &table, |vm, key, value| {
        vm.call_value(&callback, &this_arg, &[value, key, this.clone()])?;
        Ok(())
    })?;
    Ok(Value::Undefined)
}

fn map_iterator(
    vm: &mut Interpreter,
    this: &Value,
    kind: IterationKind,
    method: &str,
) -> JsResult<Value> {
    let table = this_map(vm, this, method)?;
    vm.create_table_iterator(&Slot::Map(table), kind)
}

fn map_entries(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    map_iterator(vm, this, IterationKind::Entries, "entries")
}

fn map_keys(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    map_iterator(vm, this, IterationKind::Keys, "keys")
}

fn map_values(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    map_iterator(vm, this, IterationKind::Values, "values")
}

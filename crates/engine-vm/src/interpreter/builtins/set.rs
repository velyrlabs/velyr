//! The `Set` constructor and prototype (ECMA-262 §24.2). The ES2025 set algebra methods
//! (`union`, `intersection`, ...) live in the `algebra` submodule.

mod algebra;

use std::cell::RefCell;
use std::rc::Rc;

use super::array::species_getter;
use super::collections::OrderedTable;
use super::map::for_each_entry;
use super::slots::{IterationKind, Slot};
use super::symbol::{SYMBOL_ITERATOR, SYMBOL_SPECIES};
use super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::{ObjectKey, PropertyDescriptor, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::SetPrototype);
    let constructor = vm.constructor("Set", 0, set_constructor, proto)?;
    let species = ObjectKey::Symbol(SYMBOL_SPECIES);
    vm.accessor(
        constructor,
        species,
        "[Symbol.species]",
        species_getter,
        None,
    )?;
    vm.method(proto, "add", 1, set_add)?;
    vm.method(proto, "clear", 0, set_clear)?;
    vm.method(proto, "delete", 1, set_delete)?;
    vm.method(proto, "entries", 0, set_entries)?;
    vm.method(proto, "forEach", 1, set_for_each)?;
    vm.method(proto, "has", 1, set_has)?;
    let size = vm.realm.key_str("size");
    vm.accessor(proto, size, "size", set_size, None)?;
    vm.method(proto, "values", 0, set_values)?;
    let values = vm.get_named(proto, "values")?;
    vm.define_method(proto, "keys", values.clone())?;
    let desc = PropertyDescriptor::data(values, true, false, true);
    vm.define_property(proto, ObjectKey::Symbol(SYMBOL_ITERATOR), &desc)?;
    algebra::install(vm, proto)?;
    vm.define_to_string_tag(proto, "Set")
}

/// The `[[SetData]]` of `this`.
fn this_set(
    vm: &mut Interpreter,
    this: &Value,
    method: &str,
) -> JsResult<Rc<RefCell<OrderedTable>>> {
    match vm.value_slot(this) {
        Some(Slot::Set(table)) => Ok(table),
        _ => vm.throw_type(&format!(
            "Method Set.prototype.{method} called on incompatible receiver"
        )),
    }
}

fn set_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        return vm.throw_type("Constructor Set requires 'new'");
    }
    let proto = vm.intrinsic(Intrinsic::SetPrototype);
    let table = Rc::new(RefCell::new(OrderedTable::default()));
    let set = vm.create_from_new_target(&new_target, proto, Some(Slot::Set(table)))?;
    let iterable = arg(args, 0);
    if matches!(iterable, Value::Undefined | Value::Null) {
        return Ok(object_value(set));
    }
    let adder = vm.get_named(set, "add")?;
    if !vm.is_callable(&adder) {
        return vm.throw_type("Set.prototype.add is not a function");
    }
    let target = object_value(set);
    vm.iterate(&iterable, |vm, value| {
        vm.call_value(&adder, &target, &[value])?;
        Ok(())
    })?;
    Ok(target)
}

fn set_add(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let table = this_set(vm, this, "add")?;
    let value = arg(args, 0);
    if !table.borrow().has(&value) {
        table.borrow_mut().set(value.clone(), value);
    }
    Ok(this.clone())
}

fn set_clear(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    this_set(vm, this, "clear")?.borrow_mut().clear();
    Ok(Value::Undefined)
}

fn set_delete(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let table = this_set(vm, this, "delete")?;
    let deleted = table.borrow_mut().delete(&arg(args, 0));
    Ok(Value::Bool(deleted))
}

fn set_has(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let table = this_set(vm, this, "has")?;
    let has = table.borrow().has(&arg(args, 0));
    Ok(Value::Bool(has))
}

fn set_size(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let table = this_set(vm, this, "size")?;
    let size = table.borrow().size();
    Ok(Value::Number(size as f64))
}

fn set_for_each(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let table = this_set(vm, this, "forEach")?;
    let callback = arg(args, 0);
    if !vm.is_callable(&callback) {
        return vm.throw_type("Set.prototype.forEach: callback is not a function");
    }
    let this_arg = arg(args, 1);
    for_each_entry(vm, &table, |vm, key, _| {
        vm.call_value(&callback, &this_arg, &[key.clone(), key, this.clone()])?;
        Ok(())
    })?;
    Ok(Value::Undefined)
}

fn set_iterator(
    vm: &mut Interpreter,
    this: &Value,
    kind: IterationKind,
    method: &str,
) -> JsResult<Value> {
    let table = this_set(vm, this, method)?;
    vm.create_table_iterator(&Slot::Set(table), kind)
}

fn set_entries(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    set_iterator(vm, this, IterationKind::Entries, "entries")
}

fn set_values(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    set_iterator(vm, this, IterationKind::Values, "values")
}

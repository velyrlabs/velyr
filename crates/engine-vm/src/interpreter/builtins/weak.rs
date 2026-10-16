//! The `WeakMap` and `WeakSet` constructors and prototypes (ECMA-262 §24.3, §24.4).

use std::cell::RefCell;
use std::rc::Rc;

use super::map::add_entries;
use super::slots::{Slot, WeakTable};
use super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::Value;

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::WeakMapPrototype);
    vm.constructor("WeakMap", 0, weak_map_constructor, proto)?;
    vm.method(proto, "delete", 1, weak_map_delete)?;
    vm.method(proto, "get", 1, weak_map_get)?;
    vm.method(proto, "has", 1, weak_map_has)?;
    vm.method(proto, "set", 2, weak_map_set)?;
    vm.define_to_string_tag(proto, "WeakMap")?;
    let proto = vm.intrinsic(Intrinsic::WeakSetPrototype);
    vm.constructor("WeakSet", 0, weak_set_constructor, proto)?;
    vm.method(proto, "add", 1, weak_set_add)?;
    vm.method(proto, "delete", 1, weak_set_delete)?;
    vm.method(proto, "has", 1, weak_set_has)?;
    vm.define_to_string_tag(proto, "WeakSet")
}

/// A weakly holdable key: an object or a non-registered symbol.
pub(super) enum WeakKey {
    Object(velyr_engine_gc::GcRef),
    Symbol(crate::SymbolId),
}

pub(super) fn weak_key(vm: &Interpreter, value: &Value) -> Option<WeakKey> {
    match value {
        Value::Object(r) => Some(WeakKey::Object(*r)),
        Value::Symbol(s) if vm.can_be_held_weakly(*s) => Some(WeakKey::Symbol(*s)),
        _ => None,
    }
}

impl WeakTable {
    fn get(&self, key: &WeakKey) -> Option<&Value> {
        match key {
            WeakKey::Object(r) => self.objects.get(*r),
            WeakKey::Symbol(s) => self.symbols.iter().find(|(k, _)| k == s).map(|(_, v)| v),
        }
    }

    fn insert(&mut self, key: WeakKey, value: Value) {
        match key {
            WeakKey::Object(r) => {
                self.objects.insert(r, value);
            }
            WeakKey::Symbol(s) => match self.symbols.iter_mut().find(|(k, _)| *k == s) {
                Some(entry) => entry.1 = value,
                None => self.symbols.push((s, value)),
            },
        }
    }

    fn remove(&mut self, key: &WeakKey) -> bool {
        match key {
            WeakKey::Object(r) => self.objects.remove(*r).is_some(),
            WeakKey::Symbol(s) => {
                let before = self.symbols.len();
                self.symbols.retain(|(k, _)| k != s);
                self.symbols.len() != before
            }
        }
    }
}

fn this_table(
    vm: &mut Interpreter,
    this: &Value,
    map: bool,
    method: &str,
) -> JsResult<Rc<RefCell<WeakTable>>> {
    match (vm.value_slot(this), map) {
        (Some(Slot::WeakMap(table)), true) | (Some(Slot::WeakSet(table)), false) => Ok(table),
        _ => {
            let name = if map { "WeakMap" } else { "WeakSet" };
            vm.throw_type(&format!(
                "Method {name}.prototype.{method} called on incompatible receiver"
            ))
        }
    }
}

fn construct(vm: &mut Interpreter, args: &[Value], map: bool) -> JsResult<Value> {
    let new_target = vm.new_target();
    let (name, proto, adder_name) = match map {
        true => ("WeakMap", Intrinsic::WeakMapPrototype, "set"),
        false => ("WeakSet", Intrinsic::WeakSetPrototype, "add"),
    };
    if matches!(new_target, Value::Undefined) {
        return vm.throw_type(&format!("Constructor {name} requires 'new'"));
    }
    let table = Rc::new(RefCell::new(WeakTable::default()));
    let slot = if map {
        Slot::WeakMap(table)
    } else {
        Slot::WeakSet(table)
    };
    let proto = vm.intrinsic(proto);
    let object = object_value(vm.create_from_new_target(&new_target, proto, Some(slot))?);
    let iterable = arg(args, 0);
    if matches!(iterable, Value::Undefined | Value::Null) {
        return Ok(object);
    }
    let target = vm.as_object(&object).expect("just created");
    let adder = vm.get_named(target, adder_name)?;
    if !vm.is_callable(&adder) {
        return vm.throw_type(&format!("{name}.prototype.{adder_name} is not a function"));
    }
    if map {
        add_entries(vm, &object, &iterable, &adder)?;
    } else {
        vm.iterate(&iterable, |vm, value| {
            vm.call_value(&adder, &object, &[value]).map(|_| ())
        })?;
    }
    Ok(object)
}

fn weak_map_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    construct(vm, args, true)
}

fn weak_set_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    construct(vm, args, false)
}

fn weak_map_get(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let table = this_table(vm, this, true, "get")?;
    let Some(key) = weak_key(vm, &arg(args, 0)) else {
        return Ok(Value::Undefined);
    };
    let value = table.borrow().get(&key).cloned();
    Ok(value.unwrap_or(Value::Undefined))
}

fn has(vm: &mut Interpreter, this: &Value, args: &[Value], map: bool) -> JsResult<Value> {
    let table = this_table(vm, this, map, "has")?;
    let Some(key) = weak_key(vm, &arg(args, 0)) else {
        return Ok(Value::Bool(false));
    };
    let has = table.borrow().get(&key).is_some();
    Ok(Value::Bool(has))
}

fn delete(vm: &mut Interpreter, this: &Value, args: &[Value], map: bool) -> JsResult<Value> {
    let table = this_table(vm, this, map, "delete")?;
    let Some(key) = weak_key(vm, &arg(args, 0)) else {
        return Ok(Value::Bool(false));
    };
    let removed = table.borrow_mut().remove(&key);
    Ok(Value::Bool(removed))
}

fn insert(
    vm: &mut Interpreter,
    this: &Value,
    key: &Value,
    value: Value,
    map: bool,
) -> JsResult<Value> {
    let method = if map { "set" } else { "add" };
    let table = this_table(vm, this, map, method)?;
    let Some(key) = weak_key(vm, key) else {
        return vm.throw_type(&format!(
            "Invalid value used as weak {} key",
            if map { "map" } else { "set" }
        ));
    };
    table.borrow_mut().insert(key, value);
    Ok(this.clone())
}

fn weak_map_has(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    has(vm, this, args, true)
}

fn weak_map_delete(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    delete(vm, this, args, true)
}

fn weak_map_set(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    insert(vm, this, &arg(args, 0), arg(args, 1), true)
}

fn weak_set_add(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    insert(vm, this, &arg(args, 0), Value::Bool(true), false)
}

fn weak_set_has(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    has(vm, this, args, false)
}

fn weak_set_delete(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    delete(vm, this, args, false)
}

//! ES2025 set methods: `union`, `intersection`, `difference`, `symmetricDifference`,
//! `isSubsetOf`, `isSupersetOf` and `isDisjointFrom` (ECMA-262 §24.2.4), driven by a
//! set record (`size`, `has`, `keys`) of the argument.

use std::cell::RefCell;
use std::rc::Rc;

use super::super::collections::{OrderedTable, canonical_key};
use super::super::slots::Slot;
use super::super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use super::this_set;
use crate::{ObjectRef, Value, to_boolean};

pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    vm.method(proto, "difference", 1, set_difference)?;
    vm.method(proto, "intersection", 1, set_intersection)?;
    vm.method(proto, "isDisjointFrom", 1, set_is_disjoint_from)?;
    vm.method(proto, "isSubsetOf", 1, set_is_subset_of)?;
    vm.method(proto, "isSupersetOf", 1, set_is_superset_of)?;
    vm.method(proto, "symmetricDifference", 1, set_symmetric_difference)?;
    vm.method(proto, "union", 1, set_union)
}

struct SetRecord {
    set: Value,
    size: f64,
    has: Value,
    keys: Value,
}

/// GetSetRecord.
fn set_record(vm: &mut Interpreter, other: &Value) -> JsResult<SetRecord> {
    let Some(object) = vm.as_object(other) else {
        return vm.throw_type("The argument must be a set-like object");
    };
    let raw_size = vm.get_named(object, "size")?;
    let size = vm.coerce_number(&raw_size)?;
    if size.is_nan() {
        return vm.throw_type("The 'size' of a set-like object must be a number");
    }
    let size = super::super::abstract_ops::integer_part(size);
    if size < 0.0 {
        return vm.throw_range("The 'size' of a set-like object must not be negative");
    }
    let has = vm.get_named(object, "has")?;
    if !vm.is_callable(&has) {
        return vm.throw_type("The 'has' of a set-like object must be a function");
    }
    vm.keep(&has);
    let keys = vm.get_named(object, "keys")?;
    if !vm.is_callable(&keys) {
        return vm.throw_type("The 'keys' of a set-like object must be a function");
    }
    vm.keep(&keys);
    Ok(SetRecord {
        set: other.clone(),
        size,
        has,
        keys,
    })
}

/// GetIteratorFromMethod over `keys`, with a callable `next`.
fn keys_iterator(vm: &mut Interpreter, record: &SetRecord) -> JsResult<(Value, Value)> {
    let iterator = vm.call_value(&record.keys, &record.set, &[])?;
    let Some(object) = vm.as_object(&iterator) else {
        return vm.throw_type("The 'keys' method must return an object");
    };
    vm.keep(&iterator);
    let next = vm.get_named(object, "next")?;
    if !vm.is_callable(&next) {
        return vm.throw_type("The keys iterator 'next' must be a function");
    }
    vm.keep(&next);
    Ok((iterator, next))
}

/// IteratorClose with a normal completion.
fn close_iterator(vm: &mut Interpreter, iterator: &Value) -> JsResult<()> {
    let key = vm.realm.key_str("return");
    let Some(method) = vm.get_method(iterator, key)? else {
        return Ok(());
    };
    let result = vm.call_value(&method, iterator, &[])?;
    if vm.as_object(&result).is_none() {
        return vm.throw_type("Iterator result is not an object");
    }
    Ok(())
}

/// Next value of the keys iterator, canonicalized (-0 becomes +0) and rooted.
fn next_key(vm: &mut Interpreter, iterator: &(Value, Value)) -> JsResult<Option<Value>> {
    let value = vm
        .next_value(&iterator.0, &iterator.1)?
        .map(|v| canonical_key(&v));
    if let Some(value) = &value {
        vm.keep(value);
    }
    Ok(value)
}

/// `ToBoolean(Call(has, set, [value]))`.
fn other_has(vm: &mut Interpreter, record: &SetRecord, value: &Value) -> JsResult<bool> {
    let result = vm.call_value(&record.has, &record.set, std::slice::from_ref(value))?;
    Ok(to_boolean(&result))
}

/// Walks the live entries of `table` until `f` returns `Some`.
fn walk<T>(
    vm: &mut Interpreter,
    table: &Rc<RefCell<OrderedTable>>,
    mut f: impl FnMut(&mut Interpreter, Value) -> JsResult<Option<T>>,
) -> JsResult<Option<T>> {
    let mut position = 0;
    loop {
        let entry = table.borrow().entry_from(position);
        let Some((next, key, _)) = entry else {
            return Ok(None);
        };
        position = next;
        vm.keep(&key);
        if let Some(result) = f(vm, key)? {
            return Ok(Some(result));
        }
    }
}

fn new_set(vm: &mut Interpreter, table: OrderedTable) -> JsResult<Value> {
    let proto = vm.intrinsic(Intrinsic::SetPrototype);
    let set = vm.new_slotted(proto, Slot::Set(Rc::new(RefCell::new(table))))?;
    Ok(object_value(set))
}

/// `this`'s set data, its size and the argument's set record.
fn operands(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    method: &str,
) -> JsResult<(Rc<RefCell<OrderedTable>>, f64, SetRecord)> {
    let table = this_set(vm, this, method)?;
    let record = set_record(vm, &arg(args, 0))?;
    let size = table.borrow().size() as f64;
    Ok((table, size, record))
}

fn set_union(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (table, _, record) = operands(vm, this, args, "union")?;
    let iterator = keys_iterator(vm, &record)?;
    let mut result = table.borrow().clone();
    while let Some(value) = next_key(vm, &iterator)? {
        if !result.has(&value) {
            result.set(value.clone(), value);
        }
    }
    new_set(vm, result)
}

fn set_intersection(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (table, size, record) = operands(vm, this, args, "intersection")?;
    let mut result = OrderedTable::default();
    if size <= record.size {
        walk(vm, &table, |vm, value| {
            if other_has(vm, &record, &value)? && !result.has(&value) {
                result.set(value.clone(), value);
            }
            Ok(None::<()>)
        })?;
    } else {
        let iterator = keys_iterator(vm, &record)?;
        while let Some(value) = next_key(vm, &iterator)? {
            if table.borrow().has(&value) && !result.has(&value) {
                result.set(value.clone(), value);
            }
        }
    }
    new_set(vm, result)
}

fn set_difference(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (table, size, record) = operands(vm, this, args, "difference")?;
    let result = Rc::new(RefCell::new(table.borrow().clone()));
    if size <= record.size {
        let snapshot = Rc::new(RefCell::new(result.borrow().clone()));
        walk(vm, &snapshot, |vm, value| {
            if other_has(vm, &record, &value)? {
                result.borrow_mut().delete(&value);
            }
            Ok(None::<()>)
        })?;
    } else {
        let iterator = keys_iterator(vm, &record)?;
        while let Some(value) = next_key(vm, &iterator)? {
            result.borrow_mut().delete(&value);
        }
    }
    let result = result.borrow().clone();
    new_set(vm, result)
}

fn set_symmetric_difference(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (table, _, record) = operands(vm, this, args, "symmetricDifference")?;
    let iterator = keys_iterator(vm, &record)?;
    let mut result = table.borrow().clone();
    while let Some(value) = next_key(vm, &iterator)? {
        let in_result = result.has(&value);
        if table.borrow().has(&value) {
            if in_result {
                result.delete(&value);
            }
        } else if !in_result {
            result.set(value.clone(), value);
        }
    }
    new_set(vm, result)
}

fn set_is_subset_of(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (table, size, record) = operands(vm, this, args, "isSubsetOf")?;
    if size > record.size {
        return Ok(Value::Bool(false));
    }
    let missing = walk(vm, &table, |vm, value| {
        Ok((!other_has(vm, &record, &value)?).then_some(()))
    })?;
    Ok(Value::Bool(missing.is_none()))
}

fn set_is_superset_of(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (table, size, record) = operands(vm, this, args, "isSupersetOf")?;
    if size < record.size {
        return Ok(Value::Bool(false));
    }
    let iterator = keys_iterator(vm, &record)?;
    while let Some(value) = next_key(vm, &iterator)? {
        if !table.borrow().has(&value) {
            close_iterator(vm, &iterator.0)?;
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn set_is_disjoint_from(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (table, size, record) = operands(vm, this, args, "isDisjointFrom")?;
    if size <= record.size {
        let shared = walk(vm, &table, |vm, value| {
            Ok(other_has(vm, &record, &value)?.then_some(()))
        })?;
        return Ok(Value::Bool(shared.is_none()));
    }
    let iterator = keys_iterator(vm, &record)?;
    while let Some(value) = next_key(vm, &iterator)? {
        if table.borrow().has(&value) {
            close_iterator(vm, &iterator.0)?;
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

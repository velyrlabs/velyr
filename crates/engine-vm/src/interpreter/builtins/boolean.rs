//! The `Boolean` constructor and prototype (ECMA-262 §20.3).

use super::slots::Slot;
use super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::{Value, to_boolean};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::BooleanPrototype);
    vm.set_slot(proto, Slot::Boolean(false));
    vm.constructor("Boolean", 1, boolean_constructor, proto)?;
    vm.method(proto, "toString", 0, boolean_to_string)?;
    vm.method(proto, "valueOf", 0, boolean_value_of)?;
    Ok(())
}

fn boolean_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let value = to_boolean(&arg(args, 0));
    let new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        return Ok(Value::Bool(value));
    }
    let proto = vm.intrinsic(Intrinsic::BooleanPrototype);
    let object = vm.create_from_new_target(&new_target, proto, Some(Slot::Boolean(value)))?;
    Ok(object_value(object))
}

/// thisBooleanValue.
fn this_boolean(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<bool> {
    match this {
        Value::Bool(b) => Ok(*b),
        _ => match vm.value_slot(this) {
            Some(Slot::Boolean(b)) => Ok(b),
            _ => vm.throw_type(&format!("{method} requires that 'this' be a Boolean")),
        },
    }
}

fn boolean_to_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let value = this_boolean(vm, this, "Boolean.prototype.toString")?;
    Ok(Value::str(if value { "true" } else { "false" }))
}

fn boolean_value_of(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(this_boolean(
        vm,
        this,
        "Boolean.prototype.valueOf",
    )?))
}

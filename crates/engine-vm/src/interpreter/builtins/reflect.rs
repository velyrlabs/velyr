//! The `Reflect` namespace (ECMA-262 §28.1): the object internal methods as functions.

use super::object_ops::property_to_descriptor;
use super::{Interpreter, JsResult, ToPropertyDescriptor, arg, object_value};
use crate::{ObjectRef, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let reflect = vm.new_object()?;
    let methods: [(&str, u32, super::Builtin); 13] = [
        ("apply", 3, reflect_apply),
        ("construct", 2, reflect_construct),
        ("defineProperty", 3, reflect_define_property),
        ("deleteProperty", 2, reflect_delete_property),
        ("get", 2, reflect_get),
        (
            "getOwnPropertyDescriptor",
            2,
            reflect_get_own_property_descriptor,
        ),
        ("getPrototypeOf", 1, reflect_get_prototype_of),
        ("has", 2, reflect_has),
        ("isExtensible", 1, reflect_is_extensible),
        ("ownKeys", 1, reflect_own_keys),
        ("preventExtensions", 1, reflect_prevent_extensions),
        ("set", 3, reflect_set),
        ("setPrototypeOf", 2, reflect_set_prototype_of),
    ];
    for (name, arity, f) in methods {
        vm.method(reflect, name, arity, f)?;
    }
    vm.define_to_string_tag(reflect, "Reflect")?;
    let global = vm.state.intrinsics.global;
    vm.define_method(global, "Reflect", object_value(reflect))
}

fn target(vm: &mut Interpreter, args: &[Value], method: &str) -> JsResult<ObjectRef> {
    match vm.as_object(&arg(args, 0)) {
        Some(object) => Ok(object),
        None => vm.throw_type(&format!("Reflect.{method} called on non-object")),
    }
}

fn reflect_apply(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    if !vm.is_callable(&f) {
        return vm.throw_type("Reflect.apply target is not a function");
    }
    let list = vm.list_from_array_like(&arg(args, 2))?;
    vm.call_value(&f, &arg(args, 1), &list)
}

fn reflect_construct(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    if !vm.is_constructor(&f) {
        return vm.throw_type("Reflect.construct target is not a constructor");
    }
    let new_target = args.get(2).cloned().unwrap_or_else(|| f.clone());
    if !vm.is_constructor(&new_target) {
        return vm.throw_type("Reflect.construct newTarget is not a constructor");
    }
    let list = vm.list_from_array_like(&arg(args, 1))?;
    vm.construct_value(&f, &list, &new_target)
}

fn reflect_define_property(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = target(vm, args, "defineProperty")?;
    let key = vm.object_key(&arg(args, 1))?;
    let desc = arg(args, 2).to_descriptor(vm)?;
    Ok(Value::Bool(vm.define_own(object, key, &desc)?))
}

fn reflect_delete_property(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = target(vm, args, "deleteProperty")?;
    let key = vm.object_key(&arg(args, 1))?;
    Ok(Value::Bool(vm.delete_key(object, key)?))
}

fn reflect_get(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = target(vm, args, "get")?;
    let key = vm.object_key(&arg(args, 1))?;
    let receiver = args.get(2).cloned().unwrap_or(object_value(object));
    vm.get(object, key, &receiver)
}

fn reflect_get_own_property_descriptor(
    vm: &mut Interpreter,
    _: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let object = target(vm, args, "getOwnPropertyDescriptor")?;
    let key = vm.object_key(&arg(args, 1))?;
    match vm.own_property(object, key)? {
        Some(property) => vm.descriptor_object(&property_to_descriptor(&property)),
        None => Ok(Value::Undefined),
    }
}

fn reflect_get_prototype_of(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = target(vm, args, "getPrototypeOf")?;
    Ok(vm.get_prototype(object)?.map_or(Value::Null, object_value))
}

fn reflect_has(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = target(vm, args, "has")?;
    let key = vm.object_key(&arg(args, 1))?;
    Ok(Value::Bool(vm.has_property(object, key)?))
}

fn reflect_is_extensible(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = target(vm, args, "isExtensible")?;
    Ok(Value::Bool(vm.is_extensible(object)?))
}

fn reflect_own_keys(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = target(vm, args, "ownKeys")?;
    let keys: Vec<Value> = vm
        .own_keys(object)?
        .into_iter()
        .map(|key| vm.key_to_value(key))
        .collect();
    vm.array_from_list(&keys)
}

fn reflect_prevent_extensions(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = target(vm, args, "preventExtensions")?;
    Ok(Value::Bool(vm.prevent_extensions(object)?))
}

fn reflect_set(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = target(vm, args, "set")?;
    let key = vm.object_key(&arg(args, 1))?;
    let receiver = args.get(3).cloned().unwrap_or(object_value(object));
    Ok(Value::Bool(vm.set(object, key, arg(args, 2), &receiver)?))
}

fn reflect_set_prototype_of(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = target(vm, args, "setPrototypeOf")?;
    let proto = match arg(args, 1) {
        Value::Null => None,
        Value::Object(r) => Some(ObjectRef::from_raw(r)),
        _ => return vm.throw_type("Object prototype may only be an Object or null"),
    };
    Ok(Value::Bool(vm.set_prototype(object, proto)?))
}

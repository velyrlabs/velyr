//! `Object.prototype` (ECMA-262 §20.1.3) and the Annex B accessor helpers.

use super::super::slots::Slot;
use super::super::symbol::SYMBOL_TO_STRING_TAG;
use super::super::{Interpreter, JsResult, arg, object_value};
use crate::{ObjectKey, ObjectRef, PropertyDescriptor, PropertyValue, Value};

pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    let methods: [(&str, u32, super::super::Builtin); 10] = [
        ("hasOwnProperty", 1, has_own_property),
        ("isPrototypeOf", 1, is_prototype_of),
        ("propertyIsEnumerable", 1, property_is_enumerable),
        ("toLocaleString", 0, to_locale_string),
        ("toString", 0, object_to_string),
        ("valueOf", 0, value_of),
        ("__defineGetter__", 2, define_getter),
        ("__defineSetter__", 2, define_setter),
        ("__lookupGetter__", 1, lookup_getter),
        ("__lookupSetter__", 1, lookup_setter),
    ];
    for (name, arity, f) in methods {
        vm.method(proto, name, arity, f)?;
    }
    let key = vm.realm.key_str("__proto__");
    vm.accessor(proto, key, "__proto__", proto_getter, Some(proto_setter))
}

fn has_own_property(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let key = vm.object_key(&arg(args, 0))?;
    let object = vm.coerce_object(this)?;
    Ok(Value::Bool(vm.has_own(object, key)?))
}

fn is_prototype_of(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let Some(mut cursor) = vm.as_object(&arg(args, 0)) else {
        return Ok(Value::Bool(false));
    };
    let object = vm.coerce_object(this)?;
    while let Some(proto) = vm.get_prototype(cursor)? {
        if proto == object {
            return Ok(Value::Bool(true));
        }
        cursor = proto;
    }
    Ok(Value::Bool(false))
}

fn property_is_enumerable(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let key = vm.object_key(&arg(args, 0))?;
    let object = vm.coerce_object(this)?;
    let property = vm.own_property(object, key)?;
    Ok(Value::Bool(property.is_some_and(|p| p.enumerable)))
}

fn to_locale_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    vm.invoke(this, "toString", &[])
}

fn value_of(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(object_value(vm.coerce_object(this)?))
}

pub(crate) fn object_to_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let object = match this {
        Value::Undefined => return Ok(Value::str("[object Undefined]")),
        Value::Null => return Ok(Value::str("[object Null]")),
        _ => vm.coerce_object(this)?,
    };
    let builtin_tag = if vm.is_array(&object_value(object))? {
        "Array"
    } else {
        match vm.slot(object) {
            Some(Slot::Arguments) => "Arguments",
            Some(Slot::Error) => "Error",
            Some(Slot::Boolean(_)) => "Boolean",
            Some(Slot::Number(_)) => "Number",
            Some(Slot::Date(_)) => "Date",
            Some(Slot::String(_)) => "String",
            Some(Slot::RegExp(_)) => "RegExp",
            _ if vm.is_callable(&object_value(object)) => "Function",
            _ => "Object",
        }
    };
    let tag = vm.get_key(object, ObjectKey::Symbol(SYMBOL_TO_STRING_TAG))?;
    let tag = match tag {
        Value::String(tag) => tag.to_string_lossy(),
        _ => builtin_tag.to_owned(),
    };
    Ok(Value::str(&format!("[object {tag}]")))
}

fn define_accessor_b(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    getter: bool,
) -> JsResult<Value> {
    let object = vm.coerce_object(this)?;
    let function = arg(args, 1);
    if !vm.is_callable(&function) {
        return vm.throw_type("accessor is not a function");
    }
    let desc = PropertyDescriptor {
        get: getter.then(|| function.clone()),
        set: (!getter).then_some(function),
        enumerable: Some(true),
        configurable: Some(true),
        ..PropertyDescriptor::default()
    };
    let key = vm.object_key(&arg(args, 0))?;
    vm.define_or_throw(object, key, &desc)?;
    Ok(Value::Undefined)
}

fn define_getter(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    define_accessor_b(vm, this, args, true)
}

fn define_setter(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    define_accessor_b(vm, this, args, false)
}

fn lookup_accessor(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    getter: bool,
) -> JsResult<Value> {
    let mut object = vm.coerce_object(this)?;
    let key = vm.object_key(&arg(args, 0))?;
    loop {
        if let Some(property) = vm.own_property(object, key)? {
            return Ok(match property.value {
                PropertyValue::Accessor { get, .. } if getter => get,
                PropertyValue::Accessor { set, .. } if !getter => set,
                _ => Value::Undefined,
            });
        }
        match vm.get_prototype(object)? {
            Some(proto) => object = proto,
            None => return Ok(Value::Undefined),
        }
    }
}

fn lookup_getter(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    lookup_accessor(vm, this, args, true)
}

fn lookup_setter(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    lookup_accessor(vm, this, args, false)
}

fn proto_getter(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let object = vm.coerce_object(this)?;
    Ok(vm.get_prototype(object)?.map_or(Value::Null, object_value))
}

fn proto_setter(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    vm.require_coercible(this, "Object.prototype.__proto__")?;
    let proto = match arg(args, 0) {
        Value::Null => None,
        Value::Object(r) => Some(ObjectRef::from_raw(r)),
        _ => return Ok(Value::Undefined),
    };
    let Some(object) = vm.as_object(this) else {
        return Ok(Value::Undefined);
    };
    if !vm.set_prototype(object, proto)? {
        return vm.throw_type("Object.prototype.__proto__ setter failed");
    }
    Ok(Value::Undefined)
}

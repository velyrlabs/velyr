//! The `Object` constructor and its static methods (ECMA-262 §20.1.2).

use super::slots::IterationKind;
use super::{Interpreter, Intrinsic, JsResult, ToPropertyDescriptor, arg, object_value};
use crate::{ObjectKey, ObjectRef, PropertyDescriptor, Value, same_value};

pub(super) mod prototype;

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.state.intrinsics.object_prototype;
    let constructor = vm.constructor("Object", 1, object_constructor, proto)?;
    vm.set_intrinsic(Intrinsic::ObjectConstructor, constructor);
    let statics: [(&str, u32, super::Builtin); 23] = [
        ("assign", 2, object_assign),
        ("create", 2, object_create),
        ("defineProperties", 2, object_define_properties),
        ("defineProperty", 3, object_define_property),
        ("entries", 1, object_entries),
        ("freeze", 1, object_freeze),
        ("fromEntries", 1, object_from_entries),
        (
            "getOwnPropertyDescriptor",
            2,
            object_get_own_property_descriptor,
        ),
        (
            "getOwnPropertyDescriptors",
            1,
            object_get_own_property_descriptors,
        ),
        ("getOwnPropertyNames", 1, object_get_own_property_names),
        ("getOwnPropertySymbols", 1, object_get_own_property_symbols),
        ("getPrototypeOf", 1, object_get_prototype_of),
        ("groupBy", 2, object_group_by),
        ("hasOwn", 2, object_has_own),
        ("is", 2, object_is),
        ("isExtensible", 1, object_is_extensible),
        ("isFrozen", 1, object_is_frozen),
        ("isSealed", 1, object_is_sealed),
        ("keys", 1, object_keys),
        ("preventExtensions", 1, object_prevent_extensions),
        ("seal", 1, object_seal),
        ("setPrototypeOf", 2, object_set_prototype_of),
        ("values", 1, object_values),
    ];
    for (name, arity, f) in statics {
        vm.method(constructor, name, arity, f)?;
    }
    prototype::install(vm, proto)
}

fn object_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    let this_function = object_value(vm.intrinsic(Intrinsic::ObjectConstructor));
    if !matches!(new_target, Value::Undefined) && !same_value(&new_target, &this_function) {
        let proto = vm.state.intrinsics.object_prototype;
        return Ok(object_value(vm.create_from_new_target(
            &new_target,
            proto,
            None,
        )?));
    }
    match arg(args, 0) {
        Value::Undefined | Value::Null => Ok(object_value(vm.new_object()?)),
        value => Ok(object_value(vm.coerce_object(&value)?)),
    }
}

fn object_assign(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let to = vm.coerce_object(&arg(args, 0))?;
    for source in args.iter().skip(1) {
        if matches!(source, Value::Undefined | Value::Null) {
            continue;
        }
        let from = vm.coerce_object(source)?;
        for key in vm.own_keys(from)? {
            if vm.own_property(from, key)?.is_some_and(|p| p.enumerable) {
                let value = vm.get_key(from, key)?;
                vm.set_or_throw(to, key, value)?;
            }
        }
    }
    Ok(object_value(to))
}

fn object_create(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let proto = match arg(args, 0) {
        Value::Null => None,
        Value::Object(r) => Some(ObjectRef::from_raw(r)),
        _ => return vm.throw_type("Object prototype may only be an Object or null"),
    };
    let object = vm.new_object_with(proto)?;
    let properties = arg(args, 1);
    if !matches!(properties, Value::Undefined) {
        vm.define_properties(object, &properties)?;
    }
    Ok(object_value(object))
}

impl Interpreter {
    /// ObjectDefineProperties.
    fn define_properties(&mut self, object: ObjectRef, properties: &Value) -> JsResult<()> {
        let props = self.coerce_object(properties)?;
        let mut descriptors: Vec<(ObjectKey, PropertyDescriptor)> = Vec::new();
        for key in self.own_keys(props)? {
            if !self.own_property(props, key)?.is_some_and(|p| p.enumerable) {
                continue;
            }
            let desc_object = self.get_key(props, key)?;
            self.keep(&desc_object);
            let desc = desc_object.to_descriptor(self)?;
            for value in [&desc.value, &desc.get, &desc.set].into_iter().flatten() {
                self.keep(value);
            }
            descriptors.push((key, desc));
        }
        for (key, desc) in descriptors {
            self.define_or_throw(object, key, &desc)?;
        }
        Ok(())
    }

    /// The object argument of a static method that requires one.
    fn object_argument(&mut self, value: &Value, method: &str) -> JsResult<ObjectRef> {
        match self.as_object(value) {
            Some(object) => Ok(object),
            None => self.throw_type(&format!("Object.{method} called on non-object")),
        }
    }
}

fn object_define_properties(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = vm.object_argument(&arg(args, 0), "defineProperties")?;
    vm.define_properties(object, &arg(args, 1))?;
    Ok(object_value(object))
}

fn object_define_property(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = vm.object_argument(&arg(args, 0), "defineProperty")?;
    let key = vm.object_key(&arg(args, 1))?;
    let desc = arg(args, 2).to_descriptor(vm)?;
    vm.define_or_throw(object, key, &desc)?;
    Ok(object_value(object))
}

fn enumerable_own_array(
    vm: &mut Interpreter,
    args: &[Value],
    kind: IterationKind,
) -> JsResult<Value> {
    let object = vm.coerce_object(&arg(args, 0))?;
    let list = vm.enumerable_own(object, kind)?;
    vm.array_from_list(&list)
}

fn object_entries(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    enumerable_own_array(vm, args, IterationKind::Entries)
}

fn object_keys(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    enumerable_own_array(vm, args, IterationKind::Keys)
}

fn object_values(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    enumerable_own_array(vm, args, IterationKind::Values)
}

fn integrity(vm: &mut Interpreter, args: &[Value], frozen: bool) -> JsResult<Value> {
    let value = arg(args, 0);
    let Some(object) = vm.as_object(&value) else {
        return Ok(value);
    };
    if !vm.set_integrity(object, frozen)? {
        return vm.throw_type("Cannot change the integrity level of this object");
    }
    Ok(value)
}

fn object_freeze(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    integrity(vm, args, true)
}

fn object_seal(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    integrity(vm, args, false)
}

fn test_integrity(vm: &mut Interpreter, args: &[Value], frozen: bool) -> JsResult<Value> {
    match vm.as_object(&arg(args, 0)) {
        Some(object) => Ok(Value::Bool(vm.test_integrity(object, frozen)?)),
        None => Ok(Value::Bool(true)),
    }
}

fn object_is_frozen(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    test_integrity(vm, args, true)
}

fn object_is_sealed(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    test_integrity(vm, args, false)
}

fn object_from_entries(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let iterable = arg(args, 0);
    vm.require_coercible(&iterable, "Object.fromEntries")?;
    let object = vm.new_object()?;
    vm.iterate(&iterable, |vm, entry| {
        let Some(entry_object) = vm.as_object(&entry) else {
            return vm.throw_type("Iterator value is not an entry object");
        };
        let key = vm.get_key(entry_object, ObjectKey::Index(0))?;
        vm.keep(&key);
        let value = vm.get_key(entry_object, ObjectKey::Index(1))?;
        vm.keep(&value);
        let key = vm.object_key(&key)?;
        vm.create_data_or_throw(object, key, value)
    })?;
    Ok(object_value(object))
}

fn object_get_own_property_descriptor(
    vm: &mut Interpreter,
    _: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let object = vm.coerce_object(&arg(args, 0))?;
    let key = vm.object_key(&arg(args, 1))?;
    match vm.own_property(object, key)? {
        Some(property) => {
            let desc = super::object_ops::property_to_descriptor(&property);
            vm.descriptor_object(&desc)
        }
        None => Ok(Value::Undefined),
    }
}

fn object_get_own_property_descriptors(
    vm: &mut Interpreter,
    _: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let object = vm.coerce_object(&arg(args, 0))?;
    let result = vm.new_object()?;
    for key in vm.own_keys(object)? {
        if let Some(property) = vm.own_property(object, key)? {
            let desc = super::object_ops::property_to_descriptor(&property);
            let desc = vm.descriptor_object(&desc)?;
            vm.create_data_or_throw(result, key, desc)?;
        }
    }
    Ok(object_value(result))
}

fn own_keys_of_type(vm: &mut Interpreter, args: &[Value], symbols: bool) -> JsResult<Value> {
    let object = vm.coerce_object(&arg(args, 0))?;
    let keys: Vec<Value> = vm
        .own_keys(object)?
        .into_iter()
        .filter(|key| matches!(key, ObjectKey::Symbol(_)) == symbols)
        .map(|key| vm.key_to_value(key))
        .collect();
    vm.array_from_list(&keys)
}

fn object_get_own_property_names(
    vm: &mut Interpreter,
    _: &Value,
    args: &[Value],
) -> JsResult<Value> {
    own_keys_of_type(vm, args, false)
}

fn object_get_own_property_symbols(
    vm: &mut Interpreter,
    _: &Value,
    args: &[Value],
) -> JsResult<Value> {
    own_keys_of_type(vm, args, true)
}

fn object_get_prototype_of(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = vm.coerce_object(&arg(args, 0))?;
    Ok(vm.get_prototype(object)?.map_or(Value::Null, object_value))
}

fn object_group_by(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let groups = vm.group_by(&arg(args, 0), &arg(args, 1), true)?;
    let object = vm.new_object_with(None)?;
    for (key, values) in groups {
        let array = vm.array_from_list(&values)?;
        let key = vm.object_key(&key)?;
        vm.create_data_or_throw(object, key, array)?;
    }
    Ok(object_value(object))
}

impl Interpreter {
    /// GroupBy(items, callback, property | zero): groups in first-seen key order.
    pub(crate) fn group_by(
        &mut self,
        items: &Value,
        callback: &Value,
        property_keys: bool,
    ) -> JsResult<Vec<(Value, Vec<Value>)>> {
        self.require_coercible(items, "groupBy")?;
        if !self.is_callable(callback) {
            return self.throw_type("groupBy callback is not a function");
        }
        let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
        let mut index = 0.0;
        self.iterate(items, |vm, value| {
            let key = vm.call_value(
                callback,
                &Value::Undefined,
                &[value.clone(), Value::Number(index)],
            )?;
            index += 1.0;
            let key = if property_keys {
                vm.coerce_property_key(&key)?
            } else {
                super::collections::canonical_key(&key)
            };
            vm.keep(&key);
            match groups.iter_mut().find(|(k, _)| same_value(k, &key)) {
                Some((_, values)) => values.push(value),
                None => groups.push((key, vec![value])),
            }
            Ok(())
        })?;
        Ok(groups)
    }
}

fn object_has_own(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let object = vm.coerce_object(&arg(args, 0))?;
    let key = vm.object_key(&arg(args, 1))?;
    Ok(Value::Bool(vm.has_own(object, key)?))
}

fn object_is(_: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(same_value(&arg(args, 0), &arg(args, 1))))
}

fn object_is_extensible(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    match vm.as_object(&arg(args, 0)) {
        Some(object) => Ok(Value::Bool(vm.is_extensible(object)?)),
        None => Ok(Value::Bool(false)),
    }
}

fn object_prevent_extensions(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let value = arg(args, 0);
    if let Some(object) = vm.as_object(&value)
        && !vm.prevent_extensions(object)?
    {
        return vm.throw_type("Cannot prevent extensions");
    }
    Ok(value)
}

fn object_set_prototype_of(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let value = arg(args, 0);
    vm.require_coercible(&value, "Object.setPrototypeOf")?;
    let proto = match arg(args, 1) {
        Value::Null => None,
        Value::Object(r) => Some(ObjectRef::from_raw(r)),
        _ => return vm.throw_type("Object prototype may only be an Object or null"),
    };
    if let Some(object) = vm.as_object(&value)
        && !vm.set_prototype(object, proto)?
    {
        return vm.throw_type("Cannot set the prototype of this object");
    }
    Ok(value)
}

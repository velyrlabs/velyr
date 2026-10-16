//! Proxy exotic objects (ECMA-262 §10.5) with every trap invariant, the `Proxy`
//! constructor and `Proxy.revocable`.

use std::cell::RefCell;
use std::rc::Rc;

use super::object_ops::{complete_descriptor, property_to_descriptor};
use super::slots::{ProxyState, Slot};
use super::{Interpreter, Intrinsic, JsResult, ToPropertyDescriptor, arg, object_value};
use crate::interpreter::function::Callable;
use crate::object::validate_and_apply;
use crate::{
    ObjectKey, ObjectRef, Property, PropertyDescriptor, PropertyValue, Value, same_value,
    to_boolean,
};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proxy = vm.create_native_function("Proxy", 2, Rc::new(proxy_constructor), true)?;
    vm.keep(&object_value(proxy));
    vm.set_intrinsic(Intrinsic::ProxyConstructor, proxy);
    vm.method(proxy, "revocable", 2, proxy_revocable)?;
    let global = vm.state.intrinsics.global;
    vm.define_method(global, "Proxy", object_value(proxy))
}

fn proxy_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    if matches!(vm.new_target(), Value::Undefined) {
        return vm.throw_type("Constructor Proxy requires 'new'");
    }
    let (proxy, _) = vm.proxy_create(&arg(args, 0), &arg(args, 1))?;
    Ok(object_value(proxy))
}

fn proxy_revocable(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let (proxy, state) = vm.proxy_create(&arg(args, 0), &arg(args, 1))?;
    let revoke = Rc::new(move |_: &mut Interpreter, _: &Value, _: &[Value]| {
        let mut state = state.borrow_mut();
        state.target = None;
        state.handler = None;
        Ok(Value::Undefined)
    });
    let revoke = vm.create_native_function("", 0, revoke, false)?;
    vm.keep(&object_value(revoke));
    let result = vm.new_object()?;
    let key = vm.realm.key_str("proxy");
    vm.create_data_property(result, key, object_value(proxy))?;
    let key = vm.realm.key_str("revoke");
    vm.create_data_property(result, key, object_value(revoke))?;
    Ok(object_value(result))
}

/// The [[Call]]/[[Construct]] behavior of a callable proxy (`NewTarget` tells them apart).
fn callable_proxy(proxy: ObjectRef) -> Callable {
    let call = Rc::new(move |vm: &mut Interpreter, this: &Value, args: &[Value]| {
        let new_target = vm.new_target();
        if matches!(new_target, Value::Undefined) {
            vm.proxy_call(proxy, this, args)
        } else {
            vm.proxy_construct(proxy, args, &new_target)
        }
    });
    Callable::Native {
        call,
        constructor: false,
    }
}

impl Interpreter {
    /// ProxyCreate(target, handler).
    pub(crate) fn proxy_create(
        &mut self,
        target: &Value,
        handler: &Value,
    ) -> JsResult<(ObjectRef, Rc<RefCell<ProxyState>>)> {
        let (Some(target_object), Some(handler)) =
            (self.as_object(target), self.as_object(handler))
        else {
            return self.throw_type("Cannot create proxy with a non-object as target or handler");
        };
        let proxy = self.new_object_with(None)?;
        let state = Rc::new(RefCell::new(ProxyState {
            target: Some(target_object),
            handler: Some(handler),
        }));
        self.set_slot(proxy, Slot::Proxy(state.clone()));
        if self.is_callable(target) {
            let mut callable = callable_proxy(proxy);
            if let Callable::Native { constructor, .. } = &mut callable {
                *constructor = self.is_constructor(target);
            }
            self.replace_callable(proxy, callable);
        }
        Ok((proxy, state))
    }

    /// (target, handler), or TypeError for a revoked proxy.
    pub(super) fn proxy_parts(&mut self, proxy: ObjectRef) -> JsResult<(ObjectRef, ObjectRef)> {
        let Some(Slot::Proxy(state)) = self.slot(proxy) else {
            return Err(self.internal("not a proxy"));
        };
        let state = state.borrow();
        match (state.target, state.handler) {
            (Some(target), Some(handler)) => Ok((target, handler)),
            _ => {
                drop(state);
                self.throw_type("Cannot perform operation on a revoked proxy")
            }
        }
    }

    /// (target, handler, trap) with the trap looked up by name (`None` when absent).
    fn proxy_trap(
        &mut self,
        proxy: ObjectRef,
        name: &str,
    ) -> JsResult<(ObjectRef, Value, Option<Value>)> {
        let (target, handler) = self.proxy_parts(proxy)?;
        let handler = object_value(handler);
        let key = self.realm.key_str(name);
        let trap = self.get_method(&handler, key)?;
        Ok((target, handler, trap))
    }

    pub(crate) fn proxy_get_prototype_of(
        &mut self,
        proxy: ObjectRef,
    ) -> JsResult<Option<ObjectRef>> {
        let (target, handler, trap) = self.proxy_trap(proxy, "getPrototypeOf")?;
        let Some(trap) = trap else {
            return self.get_prototype(target);
        };
        let result = self.call_value(&trap, &handler, &[object_value(target)])?;
        let proto = match result {
            Value::Object(_) => self.as_object(&result),
            Value::Null => None,
            _ => {
                return self.throw_type(
                    "'getPrototypeOf' on proxy: trap returned neither object nor null",
                );
            }
        };
        if self.is_extensible(target)? {
            return Ok(proto);
        }
        if self.get_prototype(target)? != proto {
            return self.throw_type("'getPrototypeOf' on proxy: proxy target is non-extensible but the trap did not return its actual prototype");
        }
        Ok(proto)
    }

    pub(crate) fn proxy_set_prototype_of(
        &mut self,
        proxy: ObjectRef,
        proto: Option<ObjectRef>,
    ) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_trap(proxy, "setPrototypeOf")?;
        let Some(trap) = trap else {
            return self.set_prototype(target, proto);
        };
        let proto_value = proto.map_or(Value::Null, object_value);
        let result = self.call_value(&trap, &handler, &[object_value(target), proto_value])?;
        if !to_boolean(&result) {
            return Ok(false);
        }
        if self.is_extensible(target)? {
            return Ok(true);
        }
        if self.get_prototype(target)? != proto {
            return self.throw_type("'setPrototypeOf' on proxy: trap returned truish for setting a new prototype on the non-extensible proxy target");
        }
        Ok(true)
    }

    pub(crate) fn proxy_is_extensible(&mut self, proxy: ObjectRef) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_trap(proxy, "isExtensible")?;
        let Some(trap) = trap else {
            return self.is_extensible(target);
        };
        let result = to_boolean(&self.call_value(&trap, &handler, &[object_value(target)])?);
        if result != self.is_extensible(target)? {
            return self.throw_type("'isExtensible' on proxy: trap result does not reflect extensibility of proxy target");
        }
        Ok(result)
    }

    pub(crate) fn proxy_prevent_extensions(&mut self, proxy: ObjectRef) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_trap(proxy, "preventExtensions")?;
        let Some(trap) = trap else {
            return self.prevent_extensions(target);
        };
        let result = to_boolean(&self.call_value(&trap, &handler, &[object_value(target)])?);
        if result && self.is_extensible(target)? {
            return self.throw_type("'preventExtensions' on proxy: trap returned truish but the proxy target is extensible");
        }
        Ok(result)
    }

    pub(crate) fn proxy_get_own_property(
        &mut self,
        proxy: ObjectRef,
        key: ObjectKey,
    ) -> JsResult<Option<Property>> {
        let (target, handler, trap) = self.proxy_trap(proxy, "getOwnPropertyDescriptor")?;
        let Some(trap) = trap else {
            return self.own_property(target, key);
        };
        let key_value = self.key_to_value(key);
        let result = self.call_value(&trap, &handler, &[object_value(target), key_value])?;
        if !matches!(result, Value::Object(_) | Value::Undefined) {
            return self.throw_type(
                "'getOwnPropertyDescriptor' on proxy: trap returned neither object nor undefined",
            );
        }
        let target_desc = self.own_property(target, key)?;
        if matches!(result, Value::Undefined) {
            let Some(target_desc) = target_desc else {
                return Ok(None);
            };
            if !target_desc.configurable || !self.is_extensible(target)? {
                return self.throw_type("'getOwnPropertyDescriptor' on proxy: trap returned undefined for a property that cannot be reported as non-existent");
            }
            return Ok(None);
        }
        let extensible = self.is_extensible(target)?;
        let desc = result.to_descriptor(self)?;
        let property = complete_descriptor(&desc);
        let full = property_to_descriptor(&property);
        if validate_and_apply(target_desc.as_ref(), &full, extensible).is_none() {
            return self.throw_type("'getOwnPropertyDescriptor' on proxy: trap returned a descriptor incompatible with the target property");
        }
        if !property.configurable {
            let reportable = match &target_desc {
                Some(t) if !t.configurable => {
                    !(matches!(
                        property.value,
                        PropertyValue::Data {
                            writable: false,
                            ..
                        }
                    ) && matches!(t.value, PropertyValue::Data { writable: true, .. }))
                }
                _ => false,
            };
            if !reportable {
                return self.throw_type("'getOwnPropertyDescriptor' on proxy: trap reported non-configurability for a property that is configurable or writable on the target");
            }
        }
        Ok(Some(property))
    }

    pub(crate) fn proxy_define_own_property(
        &mut self,
        proxy: ObjectRef,
        key: ObjectKey,
        desc: &PropertyDescriptor,
    ) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_trap(proxy, "defineProperty")?;
        let Some(trap) = trap else {
            return self.define_own(target, key, desc);
        };
        let desc_object = self.descriptor_object(desc)?;
        let key_value = self.key_to_value(key);
        let args = [object_value(target), key_value, desc_object];
        if !to_boolean(&self.call_value(&trap, &handler, &args)?) {
            return Ok(false);
        }
        let target_desc = self.own_property(target, key)?;
        let extensible = self.is_extensible(target)?;
        let setting_config_false = desc.configurable == Some(false);
        let Some(target_desc) = target_desc else {
            if !extensible || setting_config_false {
                return self.throw_type("'defineProperty' on proxy: trap returned truish for adding a property that cannot be added to the target");
            }
            return Ok(true);
        };
        let compatible = validate_and_apply(Some(&target_desc), desc, extensible).is_some();
        let narrowed = setting_config_false && target_desc.configurable;
        let unwritable = matches!(
            target_desc.value,
            PropertyValue::Data { writable: true, .. }
        ) && !target_desc.configurable
            && desc.writable == Some(false);
        if !compatible || narrowed || unwritable {
            return self.throw_type("'defineProperty' on proxy: trap returned truish for a definition incompatible with the target property");
        }
        Ok(true)
    }

    pub(crate) fn proxy_has(&mut self, proxy: ObjectRef, key: ObjectKey) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_trap(proxy, "has")?;
        let Some(trap) = trap else {
            return self.has_property(target, key);
        };
        let key_value = self.key_to_value(key);
        let result =
            to_boolean(&self.call_value(&trap, &handler, &[object_value(target), key_value])?);
        if !result
            && let Some(target_desc) = self.own_property(target, key)?
            && (!target_desc.configurable || !self.is_extensible(target)?)
        {
            return self.throw_type("'has' on proxy: trap returned falsish for a property that cannot be reported as non-existent");
        }
        Ok(result)
    }

    pub(crate) fn proxy_get(
        &mut self,
        proxy: ObjectRef,
        key: ObjectKey,
        receiver: &Value,
    ) -> JsResult<Value> {
        let (target, handler, trap) = self.proxy_trap(proxy, "get")?;
        let Some(trap) = trap else {
            return self.get(target, key, receiver);
        };
        let key_value = self.key_to_value(key);
        let args = [object_value(target), key_value, receiver.clone()];
        let result = self.call_value(&trap, &handler, &args)?;
        if let Some(target_desc) = self.own_property(target, key)?
            && !target_desc.configurable
        {
            let violates = match &target_desc.value {
                PropertyValue::Data {
                    value,
                    writable: false,
                } => !same_value(&result, value),
                PropertyValue::Accessor {
                    get: Value::Undefined,
                    ..
                } => !matches!(result, Value::Undefined),
                _ => false,
            };
            if violates {
                return self.throw_type("'get' on proxy: trap result is incompatible with a non-configurable target property");
            }
        }
        Ok(result)
    }

    pub(crate) fn proxy_set(
        &mut self,
        proxy: ObjectRef,
        key: ObjectKey,
        value: Value,
        receiver: &Value,
    ) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_trap(proxy, "set")?;
        let Some(trap) = trap else {
            return self.set(target, key, value, receiver);
        };
        let key_value = self.key_to_value(key);
        let args = [
            object_value(target),
            key_value,
            value.clone(),
            receiver.clone(),
        ];
        if !to_boolean(&self.call_value(&trap, &handler, &args)?) {
            return Ok(false);
        }
        if let Some(target_desc) = self.own_property(target, key)?
            && !target_desc.configurable
        {
            let violates = match &target_desc.value {
                PropertyValue::Data {
                    value: current,
                    writable: false,
                } => !same_value(&value, current),
                PropertyValue::Accessor {
                    set: Value::Undefined,
                    ..
                } => true,
                _ => false,
            };
            if violates {
                return self.throw_type("'set' on proxy: trap returned truish for a non-configurable target property that cannot change");
            }
        }
        Ok(true)
    }

    pub(crate) fn proxy_delete(&mut self, proxy: ObjectRef, key: ObjectKey) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_trap(proxy, "deleteProperty")?;
        let Some(trap) = trap else {
            return self.delete_key(target, key);
        };
        let key_value = self.key_to_value(key);
        if !to_boolean(&self.call_value(&trap, &handler, &[object_value(target), key_value])?) {
            return Ok(false);
        }
        let Some(target_desc) = self.own_property(target, key)? else {
            return Ok(true);
        };
        if !target_desc.configurable || !self.is_extensible(target)? {
            return self.throw_type("'deleteProperty' on proxy: trap returned truish for a property that cannot be deleted from the target");
        }
        Ok(true)
    }

    pub(crate) fn proxy_own_keys(&mut self, proxy: ObjectRef) -> JsResult<Vec<ObjectKey>> {
        let (target, handler, trap) = self.proxy_trap(proxy, "ownKeys")?;
        let Some(trap) = trap else {
            return self.own_keys(target);
        };
        let result = self.call_value(&trap, &handler, &[object_value(target)])?;
        let keys = self.trap_key_list(&result)?;
        let extensible = self.is_extensible(target)?;
        let mut configurable = Vec::new();
        let mut fixed = Vec::new();
        for key in self.own_keys(target)? {
            match self.own_property(target, key)? {
                Some(p) if !p.configurable => fixed.push(key),
                _ => configurable.push(key),
            }
        }
        if extensible && fixed.is_empty() {
            return Ok(keys);
        }
        let mut unchecked = keys.clone();
        let required = if extensible {
            fixed
        } else {
            fixed.into_iter().chain(configurable).collect()
        };
        for key in required {
            let Some(position) = unchecked.iter().position(|k| *k == key) else {
                return self.throw_type(
                    "'ownKeys' on proxy: trap result did not include a required key of the target",
                );
            };
            unchecked.remove(position);
        }
        if !extensible && !unchecked.is_empty() {
            return self.throw_type("'ownKeys' on proxy: trap returned extra keys but the proxy target is non-extensible");
        }
        Ok(keys)
    }

    /// CreateListFromArrayLike(result, « String, Symbol ») without duplicates.
    fn trap_key_list(&mut self, result: &Value) -> JsResult<Vec<ObjectKey>> {
        if !matches!(result, Value::Object(_)) {
            return self.throw_type("'ownKeys' on proxy: trap returned a non-object");
        }
        let mut keys = Vec::new();
        for value in self.list_from_array_like(result)? {
            let key = match &value {
                Value::String(s) => self.string_key(s),
                Value::Symbol(s) => ObjectKey::Symbol(*s),
                _ => return self.throw_type(
                    "'ownKeys' on proxy: trap result contains a value that is not a property key",
                ),
            };
            if keys.contains(&key) {
                return self.throw_type("'ownKeys' on proxy: trap returned duplicate entries");
            }
            keys.push(key);
        }
        Ok(keys)
    }

    fn proxy_call(&mut self, proxy: ObjectRef, this: &Value, args: &[Value]) -> JsResult<Value> {
        let (target, handler, trap) = self.proxy_trap(proxy, "apply")?;
        let Some(trap) = trap else {
            return self.call_value(&object_value(target), this, args);
        };
        let array = self.array_from_list(args)?;
        self.call_value(
            &trap,
            &handler,
            &[object_value(target), this.clone(), array],
        )
    }

    fn proxy_construct(
        &mut self,
        proxy: ObjectRef,
        args: &[Value],
        new_target: &Value,
    ) -> JsResult<Value> {
        let (target, handler, trap) = self.proxy_trap(proxy, "construct")?;
        let target = object_value(target);
        let Some(trap) = trap else {
            return self.construct_value(&target, args, new_target);
        };
        let array = self.array_from_list(args)?;
        let result = self.call_value(&trap, &handler, &[target, array, new_target.clone()])?;
        if !matches!(result, Value::Object(_)) {
            return self.throw_type("proxy [[Construct]] must return an object");
        }
        Ok(result)
    }
}

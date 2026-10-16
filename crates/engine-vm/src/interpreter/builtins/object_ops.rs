//! Object internal methods as the built-ins see them: ordinary objects go to the realm,
//! Proxy objects run their traps (see `proxy`). Also property descriptor conversions and
//! the throwing variants of the internal methods (ECMA-262 §7.3).

use super::slots::Slot;
use super::typed_arrays::exotic::Exotic;
use super::{Interpreter, JsResult, object_value};
use crate::interpreter::class::is_private;
use crate::{
    InternalMethods, ObjectKey, ObjectRef, Property, PropertyDescriptor, PropertyValue, Value,
    to_boolean,
};

/// ToPropertyDescriptor, as a trait so `PropertyDescriptor` keeps its plain constructors.
pub(crate) trait ToPropertyDescriptor {
    fn to_descriptor(&self, vm: &mut Interpreter) -> JsResult<PropertyDescriptor>;
}

impl ToPropertyDescriptor for Value {
    fn to_descriptor(&self, vm: &mut Interpreter) -> JsResult<PropertyDescriptor> {
        let Some(object) = vm.as_object(self) else {
            return vm.throw_type("Property description must be an object");
        };
        let mut desc = PropertyDescriptor::default();
        let field = |vm: &mut Interpreter, name: &str| -> JsResult<Option<Value>> {
            let key = vm.realm.key_str(name);
            if !vm.has_property(object, key)? {
                return Ok(None);
            }
            vm.get(object, key, self).map(Some)
        };
        desc.enumerable = field(vm, "enumerable")?.map(|v| to_boolean(&v));
        desc.configurable = field(vm, "configurable")?.map(|v| to_boolean(&v));
        desc.value = field(vm, "value")?;
        desc.writable = field(vm, "writable")?.map(|v| to_boolean(&v));
        desc.get = field(vm, "get")?;
        desc.set = field(vm, "set")?;
        for accessor in [&desc.get, &desc.set].into_iter().flatten() {
            if !matches!(accessor, Value::Undefined) && !vm.is_callable(accessor) {
                return vm.throw_type("Getter or setter must be a function");
            }
        }
        if desc.is_accessor() && desc.is_data() {
            return vm.throw_type(
                "Invalid property descriptor. Cannot both specify accessors and a value or writable attribute",
            );
        }
        Ok(desc)
    }
}

/// CompletePropertyDescriptor.
pub(crate) fn complete_descriptor(desc: &PropertyDescriptor) -> Property {
    let enumerable = desc.enumerable.unwrap_or(false);
    let configurable = desc.configurable.unwrap_or(false);
    if desc.is_accessor() {
        return Property {
            value: PropertyValue::Accessor {
                get: desc.get.clone().unwrap_or(Value::Undefined),
                set: desc.set.clone().unwrap_or(Value::Undefined),
            },
            enumerable,
            configurable,
        };
    }
    let value = desc.value.clone().unwrap_or(Value::Undefined);
    Property::data(
        value,
        desc.writable.unwrap_or(false),
        enumerable,
        configurable,
    )
}

/// A full property as a descriptor with every field present.
pub(crate) fn property_to_descriptor(property: &Property) -> PropertyDescriptor {
    match &property.value {
        PropertyValue::Data { value, writable } => PropertyDescriptor::data(
            value.clone(),
            *writable,
            property.enumerable,
            property.configurable,
        ),
        PropertyValue::Accessor { get, set } => PropertyDescriptor::accessor(
            get.clone(),
            set.clone(),
            property.enumerable,
            property.configurable,
        ),
    }
}

impl Interpreter {
    pub(crate) fn is_proxy(&self, object: ObjectRef) -> bool {
        matches!(self.slot(object), Some(Slot::Proxy(_)))
    }

    /// [[GetPrototypeOf]].
    pub(crate) fn get_prototype(&mut self, o: ObjectRef) -> JsResult<Option<ObjectRef>> {
        if self.is_proxy(o) {
            return self.proxy_get_prototype_of(o);
        }
        self.prototype_of(o)
    }

    /// [[SetPrototypeOf]].
    pub(crate) fn set_prototype(
        &mut self,
        o: ObjectRef,
        proto: Option<ObjectRef>,
    ) -> JsResult<bool> {
        if self.is_proxy(o) {
            return self.proxy_set_prototype_of(o, proto);
        }
        // Object.prototype is an immutable prototype exotic object.
        if o == self.state.intrinsics.object_prototype {
            return Ok(self.prototype_of(o)? == proto);
        }
        let result = self.realm.set_prototype_of(o, proto);
        result.map_err(|e| self.object_error(e))
    }

    /// [[IsExtensible]].
    pub(crate) fn is_extensible(&mut self, o: ObjectRef) -> JsResult<bool> {
        if self.is_proxy(o) {
            return self.proxy_is_extensible(o);
        }
        let result = self.realm.is_extensible(o);
        result.map_err(|e| self.object_error(e))
    }

    /// [[PreventExtensions]].
    pub(crate) fn prevent_extensions(&mut self, o: ObjectRef) -> JsResult<bool> {
        if self.is_proxy(o) {
            return self.proxy_prevent_extensions(o);
        }
        let result = self.realm.prevent_extensions(o);
        result.map_err(|e| self.object_error(e))
    }

    /// [[GetOwnProperty]].
    pub(crate) fn own_property(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
    ) -> JsResult<Option<Property>> {
        match self.exotic(o, key) {
            Exotic::Proxy => return self.proxy_get_own_property(o, key),
            Exotic::Element(array, index) => return Ok(Self::typed_own_property(&array, index)),
            Exotic::Ordinary => {}
        }
        let result = self.realm.get_own_property(o, key);
        result.map_err(|e| self.object_error(e))
    }

    /// [[DefineOwnProperty]].
    pub(crate) fn define_own(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        desc: &PropertyDescriptor,
    ) -> JsResult<bool> {
        match self.exotic(o, key) {
            Exotic::Proxy => self.proxy_define_own_property(o, key, desc),
            Exotic::Element(array, index) => self.typed_define(&array, index, desc),
            Exotic::Ordinary => self.define_property(o, key, desc),
        }
    }

    /// [[HasProperty]].
    pub(crate) fn has_property(&mut self, o: ObjectRef, key: ObjectKey) -> JsResult<bool> {
        let mut current = o;
        loop {
            match self.exotic(current, key) {
                Exotic::Proxy => return self.proxy_has(current, key),
                Exotic::Element(array, index) => return Ok(array.valid(index).is_some()),
                Exotic::Ordinary => {}
            }
            let own = self.realm.get_own_property(current, key);
            if own.map_err(|e| self.object_error(e))?.is_some() {
                return Ok(true);
            }
            match self.prototype_of(current)? {
                Some(proto) => current = proto,
                None => return Ok(false),
            }
        }
    }

    /// [[Delete]].
    pub(crate) fn delete_key(&mut self, o: ObjectRef, key: ObjectKey) -> JsResult<bool> {
        match self.exotic(o, key) {
            Exotic::Proxy => return self.proxy_delete(o, key),
            Exotic::Element(array, index) => return Ok(array.valid(index).is_none()),
            Exotic::Ordinary => {}
        }
        let result = self.realm.delete(o, key);
        result.map_err(|e| self.object_error(e))
    }

    /// [[OwnPropertyKeys]], without private names.
    pub(crate) fn own_keys(&mut self, o: ObjectRef) -> JsResult<Vec<ObjectKey>> {
        let mut elements = match self.slot(o) {
            Some(Slot::Proxy(_)) => return self.proxy_own_keys(o),
            Some(Slot::TypedArray(array)) => self.typed_keys(&array),
            _ => Vec::new(),
        };
        let keys = self.realm.own_property_keys(o);
        let keys = keys.map_err(|e| self.object_error(e))?;
        elements.extend(keys.into_iter().filter(|key| !is_private(*key)));
        Ok(elements)
    }

    /// HasOwnProperty.
    pub(crate) fn has_own(&mut self, o: ObjectRef, key: ObjectKey) -> JsResult<bool> {
        Ok(self.own_property(o, key)?.is_some())
    }

    /// DefinePropertyOrThrow.
    pub(crate) fn define_or_throw(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        desc: &PropertyDescriptor,
    ) -> JsResult<()> {
        if !self.define_own(o, key, desc)? {
            let message = format!("Cannot redefine property: {}", self.key_text(key));
            return self.throw_type(&message);
        }
        Ok(())
    }

    /// CreateDataPropertyOrThrow.
    pub(crate) fn create_data_or_throw(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        value: Value,
    ) -> JsResult<()> {
        let desc = PropertyDescriptor::data(value, true, true, true);
        if !self.define_own(o, key, &desc)? {
            let message = format!("Cannot add property {}", self.key_text(key));
            return self.throw_type(&message);
        }
        Ok(())
    }

    /// Set(O, P, V, true).
    pub(crate) fn set_or_throw(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        value: Value,
    ) -> JsResult<()> {
        if !self.set(o, key, value, &object_value(o))? {
            let message = format!(
                "Cannot assign to read only property '{}'",
                self.key_text(key)
            );
            return self.throw_type(&message);
        }
        Ok(())
    }

    /// DeletePropertyOrThrow.
    pub(crate) fn delete_or_throw(&mut self, o: ObjectRef, key: ObjectKey) -> JsResult<()> {
        if !self.delete_key(o, key)? {
            let message = format!("Cannot delete property '{}'", self.key_text(key));
            return self.throw_type(&message);
        }
        Ok(())
    }

    /// FromPropertyDescriptor.
    pub(crate) fn descriptor_object(&mut self, desc: &PropertyDescriptor) -> JsResult<Value> {
        let object = self.new_object()?;
        let fields = [
            ("value", desc.value.clone()),
            ("writable", desc.writable.map(Value::Bool)),
            ("get", desc.get.clone()),
            ("set", desc.set.clone()),
            ("enumerable", desc.enumerable.map(Value::Bool)),
            ("configurable", desc.configurable.map(Value::Bool)),
        ];
        for (name, value) in fields {
            if let Some(value) = value {
                let key = self.realm.key_str(name);
                self.create_data_property(object, key, value)?;
            }
        }
        Ok(object_value(object))
    }

    /// SetIntegrityLevel(O, sealed | frozen).
    pub(crate) fn set_integrity(&mut self, o: ObjectRef, frozen: bool) -> JsResult<bool> {
        if !self.prevent_extensions(o)? {
            return Ok(false);
        }
        for key in self.own_keys(o)? {
            let desc = if frozen {
                match self.own_property(o, key)? {
                    Some(p) if p.is_accessor() => PropertyDescriptor {
                        configurable: Some(false),
                        ..PropertyDescriptor::default()
                    },
                    Some(_) => PropertyDescriptor {
                        configurable: Some(false),
                        writable: Some(false),
                        ..PropertyDescriptor::default()
                    },
                    None => continue,
                }
            } else {
                PropertyDescriptor {
                    configurable: Some(false),
                    ..PropertyDescriptor::default()
                }
            };
            self.define_or_throw(o, key, &desc)?;
        }
        Ok(true)
    }

    /// TestIntegrityLevel(O, sealed | frozen).
    pub(crate) fn test_integrity(&mut self, o: ObjectRef, frozen: bool) -> JsResult<bool> {
        if self.is_extensible(o)? {
            return Ok(false);
        }
        for key in self.own_keys(o)? {
            if let Some(p) = self.own_property(o, key)? {
                if p.configurable {
                    return Ok(false);
                }
                if frozen && matches!(p.value, PropertyValue::Data { writable: true, .. }) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// The enumerable own string-keyed properties: keys, values or `[key, value]` entries.
    pub(crate) fn enumerable_own(
        &mut self,
        o: ObjectRef,
        kind: super::slots::IterationKind,
    ) -> JsResult<Vec<Value>> {
        use super::slots::IterationKind;
        let mut result = Vec::new();
        for key in self.own_keys(o)? {
            if matches!(key, ObjectKey::Symbol(_)) {
                continue;
            }
            let Some(property) = self.own_property(o, key)? else {
                continue;
            };
            if !property.enumerable {
                continue;
            }
            let name = self.key_to_value(key);
            let item = match kind {
                IterationKind::Keys => name,
                IterationKind::Values => self.get_key(o, key)?,
                IterationKind::Entries => {
                    let value = self.get_key(o, key)?;
                    self.keep(&value);
                    self.array_from_list(&[name, value])?
                }
            };
            self.keep(&item);
            result.push(item);
        }
        Ok(result)
    }
}

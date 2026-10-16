//! Property descriptors and ValidateAndApplyPropertyDescriptor (ECMA-262 §10.1.6.3).

use crate::{Value, same_value};

/// A fully populated own property.
#[derive(Debug, Clone)]
pub struct Property {
    pub value: PropertyValue,
    pub enumerable: bool,
    pub configurable: bool,
}

#[derive(Debug, Clone)]
pub enum PropertyValue {
    Data { value: Value, writable: bool },
    Accessor { get: Value, set: Value },
}

impl Property {
    pub fn data(value: Value, writable: bool, enumerable: bool, configurable: bool) -> Self {
        Self {
            value: PropertyValue::Data { value, writable },
            enumerable,
            configurable,
        }
    }

    pub fn is_accessor(&self) -> bool {
        matches!(self.value, PropertyValue::Accessor { .. })
    }

    /// Writable data property, enumerable and configurable: what `CreateDataProperty` makes.
    pub(crate) fn is_default_data(&self) -> bool {
        self.enumerable
            && self.configurable
            && matches!(self.value, PropertyValue::Data { writable: true, .. })
    }
}

/// Partial descriptor as produced by ToPropertyDescriptor; absent fields are `None`.
#[derive(Debug, Clone, Default)]
pub struct PropertyDescriptor {
    pub value: Option<Value>,
    pub writable: Option<bool>,
    pub get: Option<Value>,
    pub set: Option<Value>,
    pub enumerable: Option<bool>,
    pub configurable: Option<bool>,
}

impl PropertyDescriptor {
    pub fn data(value: Value, writable: bool, enumerable: bool, configurable: bool) -> Self {
        Self {
            value: Some(value),
            writable: Some(writable),
            enumerable: Some(enumerable),
            configurable: Some(configurable),
            ..Self::default()
        }
    }

    pub fn accessor(get: Value, set: Value, enumerable: bool, configurable: bool) -> Self {
        Self {
            get: Some(get),
            set: Some(set),
            enumerable: Some(enumerable),
            configurable: Some(configurable),
            ..Self::default()
        }
    }

    pub fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }

    pub fn is_data(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }

    pub fn is_generic(&self) -> bool {
        !self.is_accessor() && !self.is_data()
    }

    fn is_empty(&self) -> bool {
        self.is_generic() && self.enumerable.is_none() && self.configurable.is_none()
    }
}

/// Returns the property to store, or `None` when the change must be rejected.
pub(crate) fn validate_and_apply(
    current: Option<&Property>,
    desc: &PropertyDescriptor,
    extensible: bool,
) -> Option<Property> {
    let Some(current) = current else {
        return extensible.then(|| from_defaults(desc));
    };
    if desc.is_empty() {
        return Some(current.clone());
    }
    if !current.configurable && !is_allowed_on_non_configurable(current, desc) {
        return None;
    }
    Some(merge(current, desc))
}

fn from_defaults(desc: &PropertyDescriptor) -> Property {
    let enumerable = desc.enumerable.unwrap_or(false);
    let configurable = desc.configurable.unwrap_or(false);
    if desc.is_accessor() {
        return Property {
            value: accessor_from(desc, Value::Undefined, Value::Undefined),
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

fn is_allowed_on_non_configurable(current: &Property, desc: &PropertyDescriptor) -> bool {
    if desc.configurable == Some(true) {
        return false;
    }
    if desc.enumerable.is_some_and(|e| e != current.enumerable) {
        return false;
    }
    if !desc.is_generic() && desc.is_accessor() != current.is_accessor() {
        return false;
    }
    match &current.value {
        PropertyValue::Accessor { get, set } => {
            is_unchanged(desc.get.as_ref(), get) && is_unchanged(desc.set.as_ref(), set)
        }
        PropertyValue::Data { writable: true, .. } => true,
        PropertyValue::Data { value, .. } => {
            desc.writable != Some(true) && is_unchanged(desc.value.as_ref(), value)
        }
    }
}

fn is_unchanged(requested: Option<&Value>, current: &Value) -> bool {
    requested.is_none_or(|value| same_value(value, current))
}

fn merge(current: &Property, desc: &PropertyDescriptor) -> Property {
    let value = match &current.value {
        PropertyValue::Data { .. } if desc.is_accessor() => {
            accessor_from(desc, Value::Undefined, Value::Undefined)
        }
        PropertyValue::Accessor { .. } if desc.is_data() => PropertyValue::Data {
            value: desc.value.clone().unwrap_or(Value::Undefined),
            writable: desc.writable.unwrap_or(false),
        },
        PropertyValue::Data { value, writable } => PropertyValue::Data {
            value: desc.value.clone().unwrap_or_else(|| value.clone()),
            writable: desc.writable.unwrap_or(*writable),
        },
        PropertyValue::Accessor { get, set } => accessor_from(desc, get.clone(), set.clone()),
    };
    Property {
        value,
        enumerable: desc.enumerable.unwrap_or(current.enumerable),
        configurable: desc.configurable.unwrap_or(current.configurable),
    }
}

fn accessor_from(desc: &PropertyDescriptor, get: Value, set: Value) -> PropertyValue {
    PropertyValue::Accessor {
        get: desc.get.clone().unwrap_or(get),
        set: desc.set.clone().unwrap_or(set),
    }
}

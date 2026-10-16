//! `Realm`: the heap plus the tables objects need (interner, shapes), and the ordinary
//! object internal methods (ECMA-262 §10.1).

use velyr_engine_gc::{Heap, Trace};
use velyr_runtime_errors::VelyrError;

use super::descriptor::validate_and_apply;
use super::key::parse_array_index;
use super::shape::{ShapeId, ShapeTable};
use super::{
    JsObject, ObjectError, ObjectKey, ObjectKind, ObjectRef, ObjectResult, Property,
    PropertyDescriptor, PropertyValue,
};
use crate::{Interner, PropertyKey, Value};

/// Calls `function` with `this` and `args`; how accessors reach the interpreter.
pub type CallHook = fn(&mut Realm, &Value, &Value, &[Value]) -> ObjectResult<Value>;

fn no_call_hook(_: &mut Realm, _: &Value, _: &Value, _: &[Value]) -> ObjectResult<Value> {
    Err(ObjectError::Type("function calls are not available".into()))
}

/// ECMA-262 object internal methods. Implemented by `Realm`, which dispatches on the
/// object kind; Proxy traps plug in as another kind behind the same interface.
pub trait InternalMethods {
    fn get_prototype_of(&mut self, o: ObjectRef) -> ObjectResult<Option<ObjectRef>>;
    fn set_prototype_of(&mut self, o: ObjectRef, proto: Option<ObjectRef>) -> ObjectResult<bool>;
    fn is_extensible(&mut self, o: ObjectRef) -> ObjectResult<bool>;
    fn prevent_extensions(&mut self, o: ObjectRef) -> ObjectResult<bool>;
    fn get_own_property(&mut self, o: ObjectRef, key: ObjectKey) -> ObjectResult<Option<Property>>;
    fn define_own_property(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        desc: &PropertyDescriptor,
    ) -> ObjectResult<bool>;
    fn has_property(&mut self, o: ObjectRef, key: ObjectKey) -> ObjectResult<bool>;
    fn get(&mut self, o: ObjectRef, key: ObjectKey, receiver: &Value) -> ObjectResult<Value>;
    fn set(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        value: Value,
        receiver: &Value,
    ) -> ObjectResult<bool>;
    fn delete(&mut self, o: ObjectRef, key: ObjectKey) -> ObjectResult<bool>;
    fn own_property_keys(&mut self, o: ObjectRef) -> ObjectResult<Vec<ObjectKey>>;
}

pub struct Realm {
    pub heap: Heap,
    pub interner: Interner,
    pub shapes: ShapeTable,
    pub call: CallHook,
    pub(super) length_key: ObjectKey,
}

impl Default for Realm {
    fn default() -> Self {
        Self::new(Heap::default())
    }
}

impl Realm {
    pub fn new(heap: Heap) -> Self {
        let mut interner = Interner::default();
        let length_key = ObjectKey::Name(interner.intern_str("length"));
        Self {
            heap,
            interner,
            shapes: ShapeTable::default(),
            call: no_call_hook,
            length_key,
        }
    }

    pub fn key(&mut self, key: PropertyKey) -> ObjectKey {
        match key {
            PropertyKey::Symbol(symbol) => ObjectKey::Symbol(symbol),
            PropertyKey::Atom(atom) => match parse_array_index(self.interner.resolve(atom)) {
                Some(index) => ObjectKey::Index(index),
                None => ObjectKey::Name(atom),
            },
        }
    }

    pub fn key_str(&mut self, name: &str) -> ObjectKey {
        let atom = self.interner.intern_str(name);
        self.key(PropertyKey::Atom(atom))
    }

    /// OrdinaryObjectCreate. `roots` is the caller's root set, as for `Heap::alloc`.
    pub fn create_object(
        &mut self,
        proto: Option<ObjectRef>,
        roots: &dyn Trace,
    ) -> Result<ObjectRef, VelyrError> {
        self.heap
            .alloc(JsObject::new(ObjectKind::Ordinary, proto), roots)
    }

    /// ArrayCreate.
    pub fn create_array(
        &mut self,
        proto: Option<ObjectRef>,
        length: u32,
        roots: &dyn Trace,
    ) -> Result<ObjectRef, VelyrError> {
        let kind = ObjectKind::Array {
            length,
            length_writable: true,
        };
        self.heap.alloc(JsObject::new(kind, proto), roots)
    }

    /// Whether `o` is an Array exotic object.
    pub fn is_array(&self, o: ObjectRef) -> bool {
        matches!(self.object(o).kind, ObjectKind::Array { .. })
    }

    /// Current shape, or `None` in dictionary mode.
    pub fn shape_id(&self, o: ObjectRef) -> Option<ShapeId> {
        self.object(o).properties.shape_id()
    }

    pub(super) fn object(&self, o: ObjectRef) -> &JsObject {
        self.heap.get(o).expect("object handle is live")
    }

    pub(super) fn object_mut(&mut self, o: ObjectRef) -> &mut JsObject {
        self.heap.get_mut(o).expect("object handle is live")
    }

    /// Shape and slot of an own data property (writable when `require_writable`), for the inline cache.
    pub(super) fn cacheable_slot(
        &self,
        o: ObjectRef,
        key: ObjectKey,
        require_writable: bool,
    ) -> Option<(ShapeId, u32)> {
        let properties = &self.object(o).properties;
        let shape = properties.shape_id()?;
        let (slot, attributes) = self.shapes.lookup(shape, key)?;
        let is_data = matches!(
            properties.slot(slot),
            Some(super::storage::Stored::Value(_))
        );
        (is_data && (attributes.writable || !require_writable)).then_some((shape, slot))
    }

    pub(super) fn ordinary_get_own_property(
        &self,
        o: ObjectRef,
        key: ObjectKey,
    ) -> Option<Property> {
        let object = self.object(o);
        match key {
            ObjectKey::Index(index) => object.elements.get(index),
            _ => object.properties.get(&self.shapes, key),
        }
    }

    pub(super) fn ordinary_define_own_property(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        desc: &PropertyDescriptor,
    ) -> bool {
        let current = self.ordinary_get_own_property(o, key);
        let extensible = self.object(o).extensible;
        let Some(property) = validate_and_apply(current.as_ref(), desc, extensible) else {
            return false;
        };
        let object = self.heap.get_mut(o).expect("object handle is live");
        match key {
            ObjectKey::Index(index) => object.elements.put(index, property),
            _ => object.properties.put(&mut self.shapes, key, property),
        }
        object.recharge();
        true
    }

    fn remove_own(&mut self, o: ObjectRef, key: ObjectKey) {
        let object = self.heap.get_mut(o).expect("object handle is live");
        match key {
            ObjectKey::Index(index) => object.elements.remove(index),
            _ => object.properties.remove(&mut self.shapes, key),
        }
        object.recharge();
    }

    fn call_accessor(
        &mut self,
        function: &Value,
        this: &Value,
        args: &[Value],
    ) -> ObjectResult<Value> {
        let call = self.call;
        call(self, function, this, args)
    }

    /// OrdinarySetWithOwnDescriptor, steps 2.c-2.e: write to the receiver.
    fn set_on_receiver(
        &mut self,
        key: ObjectKey,
        value: Value,
        receiver: &Value,
    ) -> ObjectResult<bool> {
        let Value::Object(receiver) = receiver else {
            return Ok(false);
        };
        let receiver = ObjectRef::from_raw(*receiver);
        let desc = match self.get_own_property(receiver, key)? {
            Some(Property {
                value: PropertyValue::Data { writable: true, .. },
                ..
            }) => PropertyDescriptor {
                value: Some(value),
                ..PropertyDescriptor::default()
            },
            Some(_) => return Ok(false),
            None => PropertyDescriptor::data(value, true, true, true),
        };
        self.define_own_property(receiver, key, &desc)
    }
}

impl InternalMethods for Realm {
    fn get_prototype_of(&mut self, o: ObjectRef) -> ObjectResult<Option<ObjectRef>> {
        Ok(self.object(o).prototype)
    }

    /// OrdinarySetPrototypeOf.
    fn set_prototype_of(&mut self, o: ObjectRef, proto: Option<ObjectRef>) -> ObjectResult<bool> {
        let object = self.object(o);
        if object.prototype == proto {
            return Ok(true);
        }
        if !object.extensible {
            return Ok(false);
        }
        let mut cursor = proto;
        while let Some(p) = cursor {
            if p == o {
                return Ok(false);
            }
            // A Proxy kind ends the walk here (its [[GetPrototypeOf]] is not ordinary).
            cursor = self.object(p).prototype;
        }
        self.object_mut(o).prototype = proto;
        Ok(true)
    }

    fn is_extensible(&mut self, o: ObjectRef) -> ObjectResult<bool> {
        Ok(self.object(o).extensible)
    }

    fn prevent_extensions(&mut self, o: ObjectRef) -> ObjectResult<bool> {
        self.object_mut(o).extensible = false;
        Ok(true)
    }

    fn get_own_property(&mut self, o: ObjectRef, key: ObjectKey) -> ObjectResult<Option<Property>> {
        if let ObjectKind::Array {
            length,
            length_writable,
        } = self.object(o).kind
            && key == self.length_key
        {
            let length = Value::Number(f64::from(length));
            return Ok(Some(Property::data(length, length_writable, false, false)));
        }
        if let ObjectKind::Arguments(_) = self.object(o).kind {
            return Ok(self.arguments_get_own_property(o, key));
        }
        Ok(self.ordinary_get_own_property(o, key))
    }

    fn define_own_property(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        desc: &PropertyDescriptor,
    ) -> ObjectResult<bool> {
        match self.object(o).kind {
            ObjectKind::Array { .. } => self.array_define_own_property(o, key, desc),
            ObjectKind::Ordinary => Ok(self.ordinary_define_own_property(o, key, desc)),
            ObjectKind::Arguments(_) => Ok(self.arguments_define_own_property(o, key, desc)),
        }
    }

    fn has_property(&mut self, o: ObjectRef, key: ObjectKey) -> ObjectResult<bool> {
        if self.get_own_property(o, key)?.is_some() {
            return Ok(true);
        }
        match self.get_prototype_of(o)? {
            Some(proto) => self.has_property(proto, key),
            None => Ok(false),
        }
    }

    fn get(&mut self, o: ObjectRef, key: ObjectKey, receiver: &Value) -> ObjectResult<Value> {
        let Some(property) = self.get_own_property(o, key)? else {
            return match self.get_prototype_of(o)? {
                Some(proto) => self.get(proto, key, receiver),
                None => Ok(Value::Undefined),
            };
        };
        match property.value {
            PropertyValue::Data { value, .. } => Ok(value),
            PropertyValue::Accessor {
                get: Value::Undefined,
                ..
            } => Ok(Value::Undefined),
            PropertyValue::Accessor { get, .. } => self.call_accessor(&get, receiver, &[]),
        }
    }

    /// OrdinarySet / OrdinarySetWithOwnDescriptor.
    fn set(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        value: Value,
        receiver: &Value,
    ) -> ObjectResult<bool> {
        let Some(property) = self.get_own_property(o, key)? else {
            return match self.get_prototype_of(o)? {
                Some(proto) => self.set(proto, key, value, receiver),
                None => self.set_on_receiver(key, value, receiver),
            };
        };
        match property.value {
            PropertyValue::Data {
                writable: false, ..
            } => Ok(false),
            PropertyValue::Data { .. } => self.set_on_receiver(key, value, receiver),
            PropertyValue::Accessor {
                set: Value::Undefined,
                ..
            } => Ok(false),
            PropertyValue::Accessor { set, .. } => {
                self.call_accessor(&set, receiver, &[value])?;
                Ok(true)
            }
        }
    }

    fn delete(&mut self, o: ObjectRef, key: ObjectKey) -> ObjectResult<bool> {
        match self.get_own_property(o, key)? {
            None => Ok(true),
            Some(property) if property.configurable => {
                self.remove_own(o, key);
                self.arguments_deleted(o, key);
                Ok(true)
            }
            Some(_) => Ok(false),
        }
    }

    /// OrdinaryOwnPropertyKeys: array indices ascending, then strings, then symbols, each
    /// in creation order.
    fn own_property_keys(&mut self, o: ObjectRef) -> ObjectResult<Vec<ObjectKey>> {
        let object = self.object(o);
        let mut keys: Vec<ObjectKey> = object
            .elements
            .indices()
            .into_iter()
            .map(ObjectKey::Index)
            .collect();
        if matches!(object.kind, ObjectKind::Array { .. }) {
            keys.push(self.length_key);
        }
        let (symbols, names): (Vec<ObjectKey>, Vec<ObjectKey>) = object
            .properties
            .keys(&self.shapes)
            .into_iter()
            .partition(|key| matches!(key, ObjectKey::Symbol(_)));
        keys.extend(names);
        keys.extend(symbols);
        Ok(keys)
    }
}

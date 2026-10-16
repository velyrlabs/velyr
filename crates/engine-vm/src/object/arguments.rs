//! Arguments exotic objects (ECMA-262 §10.4.4): the [[ParameterMap]] aliases mapped indices
//! to the parameter bindings, which the compiler keeps in context slots.

use velyr_engine_gc::{Gc, GcRef, Trace, Tracer};

use super::{ObjectKey, ObjectKind, ObjectRef, Property, PropertyDescriptor, PropertyValue, Realm};
use crate::Value;
use crate::interpreter::environment::Context;

/// Argument index -> context slot of its parameter, for one context.
pub(crate) struct ParameterMap {
    context: GcRef,
    slots: Vec<Option<u32>>,
}

impl ParameterMap {
    pub(crate) fn new(context: GcRef) -> Self {
        Self {
            context,
            slots: Vec::new(),
        }
    }
}

impl Trace for ParameterMap {
    fn trace(&self, tracer: &mut Tracer) {
        tracer.mark(self.context);
    }
}

impl Realm {
    /// Turns the arguments object `o` into an arguments exotic object using `map`.
    pub(crate) fn set_parameter_map(&mut self, o: ObjectRef, map: Gc<ParameterMap>) {
        self.object_mut(o).kind = ObjectKind::Arguments(map);
    }

    pub(crate) fn parameter_map(&self, o: ObjectRef) -> Option<Gc<ParameterMap>> {
        match self.object(o).kind {
            ObjectKind::Arguments(map) => Some(map),
            _ => None,
        }
    }

    /// Maps argument `index` to context slot `slot`.
    pub(crate) fn map_argument(&mut self, map: Gc<ParameterMap>, index: u32, slot: u32) {
        let map = self.heap.get_mut(map).expect("parameter map is live");
        let index = index as usize;
        if map.slots.len() <= index {
            map.slots.resize(index + 1, None);
        }
        map.slots[index] = Some(slot);
    }

    fn mapped(&self, o: ObjectRef, key: ObjectKey) -> Option<(Gc<Context>, u32)> {
        let (ObjectKind::Arguments(map), ObjectKey::Index(index)) = (self.object(o).kind, key)
        else {
            return None;
        };
        let map = self.heap.get(map)?;
        let slot = map.slots.get(index as usize).copied().flatten()?;
        Some((Gc::from_raw(map.context), slot))
    }

    fn unmap(&mut self, o: ObjectRef, key: ObjectKey) {
        if let (ObjectKind::Arguments(map), ObjectKey::Index(index)) = (self.object(o).kind, key)
            && let Some(map) = self.heap.get_mut(map)
            && let Some(slot) = map.slots.get_mut(index as usize)
        {
            *slot = None;
        }
    }

    fn mapped_value(&self, (context, slot): (Gc<Context>, u32)) -> Value {
        let context = self.heap.get(context).expect("context is live");
        context.slots[slot as usize].clone()
    }

    fn set_mapped_value(&mut self, (context, slot): (Gc<Context>, u32), value: Value) {
        let context = self.heap.get_mut(context).expect("context is live");
        context.slots[slot as usize] = value;
    }

    /// [[GetOwnProperty]]: a mapped index reads its parameter.
    pub(super) fn arguments_get_own_property(
        &self,
        o: ObjectRef,
        key: ObjectKey,
    ) -> Option<Property> {
        let mut property = self.ordinary_get_own_property(o, key)?;
        if let Some(mapping) = self.mapped(o, key)
            && let PropertyValue::Data { value, .. } = &mut property.value
        {
            *value = self.mapped_value(mapping);
        }
        Some(property)
    }

    /// [[DefineOwnProperty]]: writes go to the parameter; accessors and non-writable
    /// redefinitions end the mapping.
    pub(super) fn arguments_define_own_property(
        &mut self,
        o: ObjectRef,
        key: ObjectKey,
        desc: &PropertyDescriptor,
    ) -> bool {
        let mapping = self.mapped(o, key);
        let mut own_desc = desc.clone();
        if let Some(mapping) = mapping
            && desc.is_data()
            && desc.value.is_none()
            && desc.writable == Some(false)
        {
            own_desc.value = Some(self.mapped_value(mapping));
        }
        if !self.ordinary_define_own_property(o, key, &own_desc) {
            return false;
        }
        let Some(mapping) = mapping else {
            return true;
        };
        if desc.is_accessor() {
            self.unmap(o, key);
            return true;
        }
        if let Some(value) = &desc.value {
            self.set_mapped_value(mapping, value.clone());
        }
        if desc.writable == Some(false) {
            self.unmap(o, key);
        }
        true
    }

    /// The mapping ends with a successful [[Delete]].
    pub(super) fn arguments_deleted(&mut self, o: ObjectRef, key: ObjectKey) {
        self.unmap(o, key);
    }
}

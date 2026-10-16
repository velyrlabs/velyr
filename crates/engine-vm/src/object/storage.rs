//! Own-property storage: named properties (shape + slots, or dictionary) and indexed
//! elements (dense with holes, or sparse).

use std::collections::{BTreeMap, HashMap};

use velyr_engine_gc::{Trace, Tracer};

use super::shape::{Attributes, ShapeId, ShapeTable};
use super::{ObjectKey, Property, PropertyValue};
use crate::Value;

/// Deletions an object survives in shape mode before it switches to dictionary mode.
const DICTIONARY_DELETE_THRESHOLD: u32 = 8;
/// A write further than this past the dense end makes the elements sparse.
const MAX_DENSE_GAP: usize = 1024;

/// Slot payload in shape mode; the attributes live in the shape. Accessors are rare, so
/// they are boxed to keep every data slot at the size of a `Value`.
#[derive(Debug, Clone)]
pub(crate) enum Stored {
    Value(Value),
    /// Getter and setter.
    Accessor(Box<[Value; 2]>),
}

const _: () = assert!(size_of::<Stored>() == size_of::<Value>());

fn split(property: Property) -> (Stored, Attributes) {
    let (stored, writable) = match property.value {
        PropertyValue::Data { value, writable } => (Stored::Value(value), writable),
        PropertyValue::Accessor { get, set } => (Stored::Accessor(Box::new([get, set])), false),
    };
    let attributes = Attributes {
        writable,
        enumerable: property.enumerable,
        configurable: property.configurable,
    };
    (stored, attributes)
}

fn join(stored: &Stored, attributes: Attributes) -> Property {
    let value = match stored {
        Stored::Value(value) => PropertyValue::Data {
            value: value.clone(),
            writable: attributes.writable,
        },
        Stored::Accessor(pair) => PropertyValue::Accessor {
            get: pair[0].clone(),
            set: pair[1].clone(),
        },
    };
    Property {
        value,
        enumerable: attributes.enumerable,
        configurable: attributes.configurable,
    }
}

#[derive(Debug)]
#[allow(clippy::box_collection)] // boxed on purpose: it keeps every object cell small
pub(crate) enum NamedProperties {
    Shaped {
        shape: ShapeId,
        slots: Vec<Stored>,
        deletions: u32,
    },
    /// Insertion-ordered; deleted entries leave a `None` tombstone. Both tables are boxed
    /// so this variant fits beside `Shaped` (dictionary objects are rare).
    Dictionary {
        entries: Box<Vec<Option<(ObjectKey, Property)>>>,
        positions: Box<HashMap<ObjectKey, usize>>,
    },
}

const _: () = assert!(size_of::<NamedProperties>() == 32);
const _: () = assert!(size_of::<Elements>() == 24);

impl Default for NamedProperties {
    fn default() -> Self {
        Self::Shaped {
            shape: ShapeTable::ROOT,
            slots: Vec::new(),
            deletions: 0,
        }
    }
}

impl NamedProperties {
    pub fn shape_id(&self) -> Option<ShapeId> {
        match self {
            Self::Shaped { shape, .. } => Some(*shape),
            Self::Dictionary { .. } => None,
        }
    }

    pub fn slot(&self, slot: u32) -> Option<&Stored> {
        match self {
            Self::Shaped { slots, .. } => slots.get(slot as usize),
            Self::Dictionary { .. } => None,
        }
    }

    pub fn slot_mut(&mut self, slot: u32) -> Option<&mut Stored> {
        match self {
            Self::Shaped { slots, .. } => slots.get_mut(slot as usize),
            Self::Dictionary { .. } => None,
        }
    }

    pub fn get(&self, shapes: &ShapeTable, key: ObjectKey) -> Option<Property> {
        match self {
            Self::Shaped { shape, slots, .. } => {
                let (slot, attributes) = shapes.lookup(*shape, key)?;
                Some(join(&slots[slot as usize], attributes))
            }
            Self::Dictionary { entries, positions } => {
                let (_, property) = entries[*positions.get(&key)?].as_ref()?;
                Some(property.clone())
            }
        }
    }

    /// Inserts `key` at the end, or replaces it in place.
    pub fn put(&mut self, shapes: &mut ShapeTable, key: ObjectKey, property: Property) {
        match self {
            Self::Shaped { shape, slots, .. } => {
                let (stored, attributes) = split(property);
                match shapes.lookup(*shape, key) {
                    Some((slot, current)) => {
                        if current != attributes {
                            *shape = shapes.reconfigure(*shape, key, attributes);
                        }
                        slots[slot as usize] = stored;
                    }
                    None => {
                        *shape = shapes.add(*shape, key, attributes);
                        slots.push(stored);
                    }
                }
            }
            Self::Dictionary { entries, positions } => match positions.get(&key) {
                Some(position) => entries[*position] = Some((key, property)),
                None => {
                    positions.insert(key, entries.len());
                    entries.push(Some((key, property)));
                }
            },
        }
    }

    pub fn remove(&mut self, shapes: &mut ShapeTable, key: ObjectKey) {
        if let Self::Shaped { deletions, .. } = self {
            *deletions += 1;
            if *deletions >= DICTIONARY_DELETE_THRESHOLD {
                *self = self.to_dictionary(shapes);
            }
        }
        match self {
            Self::Shaped { shape, slots, .. } => {
                (*shape, *slots) = rebuild_without(shapes, *shape, slots, key);
            }
            Self::Dictionary { entries, positions } => {
                if let Some(position) = positions.remove(&key) {
                    entries[position] = None;
                }
            }
        }
    }

    fn to_dictionary(&self, shapes: &ShapeTable) -> Self {
        let keys = self.keys(shapes);
        let positions = Box::new(keys.iter().enumerate().map(|(i, key)| (*key, i)).collect());
        let entries = keys
            .into_iter()
            .map(|key| Some((key, self.get(shapes, key).expect("key listed by the shape"))))
            .collect();
        Self::Dictionary {
            entries: Box::new(entries),
            positions,
        }
    }

    /// Bytes held outside the object cell.
    pub fn bytes(&self) -> usize {
        match self {
            Self::Shaped { slots, .. } => slots.capacity() * size_of::<Stored>(),
            Self::Dictionary { entries, positions } => {
                entries.capacity() * size_of::<Option<(ObjectKey, Property)>>()
                    + size_of::<Vec<()>>()
                    + size_of::<HashMap<ObjectKey, usize>>()
                    + positions.capacity() * (size_of::<(ObjectKey, usize)>() + 1)
            }
        }
    }

    /// Keys in insertion order.
    pub fn keys(&self, shapes: &ShapeTable) -> Vec<ObjectKey> {
        match self {
            Self::Shaped { shape, .. } => shapes.layout(*shape).iter().map(|(k, _)| *k).collect(),
            Self::Dictionary { entries, .. } => entries.iter().flatten().map(|(k, _)| *k).collect(),
        }
    }
}

/// Replays the remaining keys from the root so the result shares shapes with objects
/// built in the same order.
fn rebuild_without(
    shapes: &mut ShapeTable,
    shape: ShapeId,
    slots: &[Stored],
    removed: ObjectKey,
) -> (ShapeId, Vec<Stored>) {
    let layout = shapes.layout(shape).to_vec();
    let mut rebuilt = ShapeTable::ROOT;
    let mut kept = Vec::with_capacity(slots.len());
    for ((key, attributes), stored) in layout.into_iter().zip(slots) {
        if key == removed {
            continue;
        }
        rebuilt = shapes.add(rebuilt, key, attributes);
        kept.push(stored.clone());
    }
    (rebuilt, kept)
}

#[derive(Debug)]
#[allow(clippy::box_collection)] // boxed on purpose: it keeps every object cell small
pub(crate) enum Elements {
    /// Every present element is a writable, enumerable, configurable data property.
    Dense(Vec<Option<Value>>),
    /// Boxed: sparse arrays are rare and the box keeps every object cell smaller.
    Sparse(Box<BTreeMap<u32, Property>>),
}

impl Default for Elements {
    fn default() -> Self {
        Self::Dense(Vec::new())
    }
}

impl Elements {
    pub fn get(&self, index: u32) -> Option<Property> {
        match self {
            Self::Dense(values) => {
                let value = values.get(index as usize)?.clone()?;
                Some(Property::data(value, true, true, true))
            }
            Self::Sparse(map) => map.get(&index).cloned(),
        }
    }

    pub fn put(&mut self, index: u32, property: Property) {
        if let Self::Dense(values) = self {
            let position = index as usize;
            if property.is_default_data() && position <= values.len() + MAX_DENSE_GAP {
                let PropertyValue::Data { value, .. } = property.value else {
                    unreachable!("default data property");
                };
                if position >= values.len() {
                    values.resize(position + 1, None);
                }
                values[position] = Some(value);
                return;
            }
            *self = self.to_sparse();
        }
        if let Self::Sparse(map) = self {
            map.insert(index, property);
        }
    }

    fn to_sparse(&self) -> Self {
        let indices = self.indices();
        Self::Sparse(Box::new(
            indices
                .into_iter()
                .filter_map(|i| Some((i, self.get(i)?)))
                .collect(),
        ))
    }

    pub fn remove(&mut self, index: u32) {
        match self {
            Self::Dense(values) => {
                if let Some(slot) = values.get_mut(index as usize) {
                    *slot = None;
                }
            }
            Self::Sparse(map) => {
                map.remove(&index);
            }
        }
    }

    /// Bytes held outside the object cell; sparse nodes are estimated at one entry each.
    pub fn bytes(&self) -> usize {
        match self {
            Self::Dense(values) => values.capacity() * size_of::<Option<Value>>(),
            Self::Sparse(map) => {
                size_of::<BTreeMap<u32, Property>>() + map.len() * size_of::<(u32, Property)>()
            }
        }
    }

    /// Present indices, ascending.
    pub fn indices(&self) -> Vec<u32> {
        match self {
            Self::Dense(values) => (0..values.len())
                .filter(|i| values[*i].is_some())
                .map(|i| i as u32)
                .collect(),
            Self::Sparse(map) => map.keys().copied().collect(),
        }
    }

    /// Deletes every index >= `new_length`, highest first. Stops at the first
    /// non-configurable element and returns its index.
    pub fn truncate(&mut self, new_length: u32) -> Result<(), u32> {
        match self {
            Self::Dense(values) => {
                values.truncate(new_length as usize);
                Ok(())
            }
            Self::Sparse(map) => {
                let doomed: Vec<u32> = map.range(new_length..).map(|(i, _)| *i).rev().collect();
                for index in doomed {
                    if !map[&index].configurable {
                        return Err(index);
                    }
                    map.remove(&index);
                }
                Ok(())
            }
        }
    }
}

impl Trace for Stored {
    fn trace(&self, tracer: &mut Tracer) {
        match self {
            Stored::Value(value) => value.trace(tracer),
            Stored::Accessor(pair) => pair.iter().for_each(|value| value.trace(tracer)),
        }
    }
}

impl Trace for Property {
    fn trace(&self, tracer: &mut Tracer) {
        match &self.value {
            PropertyValue::Data { value, .. } => value.trace(tracer),
            PropertyValue::Accessor { get, set } => (get, set).trace(tracer),
        }
    }
}

impl Trace for NamedProperties {
    fn trace(&self, tracer: &mut Tracer) {
        match self {
            Self::Shaped { slots, .. } => slots.trace(tracer),
            Self::Dictionary { entries, .. } => {
                entries
                    .iter()
                    .flatten()
                    .for_each(|(_, property)| property.trace(tracer));
            }
        }
    }
}

impl Trace for Elements {
    fn trace(&self, tracer: &mut Tracer) {
        match self {
            Self::Dense(values) => values.trace(tracer),
            Self::Sparse(map) => map.values().for_each(|property| property.trace(tracer)),
        }
    }
}

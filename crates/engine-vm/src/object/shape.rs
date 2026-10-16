//! Shared shapes (hidden classes): property layout plus a transition tree keyed by
//! property addition and attribute reconfiguration.

use std::collections::HashMap;

use velyr_engine_gc::external::{Charge, Kind};

use super::ObjectKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShapeId(u32);

/// Attribute bits kept in the shape. Data vs accessor lives in the slot, so the inline
/// cache re-checks the slot kind on a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Attributes {
    pub writable: bool,
    pub enumerable: bool,
    pub configurable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Transition {
    Add(ObjectKey, Attributes),
    Reconfigure(ObjectKey, Attributes),
}

#[derive(Debug, Default)]
struct Shape {
    /// Slot `i` of an object with this shape holds `layout[i]`.
    layout: Vec<(ObjectKey, Attributes)>,
    slots: HashMap<ObjectKey, u32>,
    transitions: HashMap<Transition, ShapeId>,
}

/// Owns every shape of a realm.
/// shortcut: shapes are never collected and each copies its parent's layout (O(n^2) for
/// long chains); add a shape GC / parent-linked layout once profiles show it matters.
#[derive(Debug)]
pub struct ShapeTable {
    shapes: Vec<Shape>,
    /// Estimated table bytes (hash maps by capacity), as `Kind::Shapes`.
    charge: Charge,
}

impl Default for ShapeTable {
    fn default() -> Self {
        Self {
            shapes: vec![Shape::default()],
            charge: Charge::new(Kind::Shapes, size_of::<Shape>()),
        }
    }
}

impl Shape {
    /// Owned bytes of the layout and both maps; a map entry costs its pair plus a control byte.
    fn bytes(&self) -> usize {
        self.layout.capacity() * size_of::<(ObjectKey, Attributes)>()
            + self.slots.capacity() * (size_of::<(ObjectKey, u32)>() + 1)
            + self.transitions.capacity() * (size_of::<(Transition, ShapeId)>() + 1)
    }
}

impl ShapeTable {
    pub const ROOT: ShapeId = ShapeId(0);

    fn shape(&self, id: ShapeId) -> &Shape {
        &self.shapes[id.0 as usize]
    }

    pub(crate) fn lookup(&self, id: ShapeId, key: ObjectKey) -> Option<(u32, Attributes)> {
        let shape = self.shape(id);
        let slot = *shape.slots.get(&key)?;
        Some((slot, shape.layout[slot as usize].1))
    }

    pub(crate) fn layout(&self, id: ShapeId) -> &[(ObjectKey, Attributes)] {
        &self.shape(id).layout
    }

    /// Shape with `key` appended as the next slot.
    pub(crate) fn add(&mut self, from: ShapeId, key: ObjectKey, attributes: Attributes) -> ShapeId {
        self.transition(from, Transition::Add(key, attributes), |layout| {
            layout.push((key, attributes));
        })
    }

    /// Same layout with the attributes of `key` replaced.
    pub(crate) fn reconfigure(
        &mut self,
        from: ShapeId,
        key: ObjectKey,
        attributes: Attributes,
    ) -> ShapeId {
        self.transition(from, Transition::Reconfigure(key, attributes), |layout| {
            let entry = layout.iter_mut().find(|(k, _)| *k == key);
            entry.expect("reconfigured key is in the shape").1 = attributes;
        })
    }

    fn transition(
        &mut self,
        from: ShapeId,
        transition: Transition,
        edit: impl FnOnce(&mut Vec<(ObjectKey, Attributes)>),
    ) -> ShapeId {
        if let Some(target) = self.shape(from).transitions.get(&transition) {
            return *target;
        }
        let mut layout = self.shape(from).layout.clone();
        edit(&mut layout);
        let slots = layout
            .iter()
            .enumerate()
            .map(|(slot, (key, _))| (*key, slot as u32))
            .collect();
        let id = ShapeId(u32::try_from(self.shapes.len()).expect("shape table overflow"));
        let shape = Shape {
            layout,
            slots,
            transitions: HashMap::new(),
        };
        let parent_before = self.shape(from).bytes();
        let table_before = self.shapes.capacity();
        let mut added = shape.bytes();
        self.shapes.push(shape);
        let parent = &mut self.shapes[from.0 as usize];
        parent.transitions.insert(transition, id);
        added += parent.bytes() - parent_before;
        added += (self.shapes.capacity() - table_before) * size_of::<Shape>();
        self.charge.grow(added);
        id
    }
}

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

/// Property layout shared along a chain of `Add` transitions (like V8's descriptor
/// arrays): a shape sees the first `len` entries, and the shape at the tip of the chain
/// extends the layout in place instead of copying it.
#[derive(Debug, Default)]
struct Layout {
    entries: Vec<(ObjectKey, Attributes)>,
    slots: HashMap<ObjectKey, u32>,
}

#[derive(Debug, Default)]
struct Shape {
    /// Index into `ShapeTable::layouts`; slot `i < len` holds `entries[i]`.
    layout: u32,
    len: u32,
    transitions: HashMap<Transition, ShapeId>,
}

/// Owns every shape of a realm.
/// shortcut: shapes are never collected; add a shape GC once profiles show it matters.
#[derive(Debug)]
pub struct ShapeTable {
    shapes: Vec<Shape>,
    layouts: Vec<Layout>,
    /// Estimated table bytes (hash maps by capacity), as `Kind::Shapes`.
    charge: Charge,
}

impl Default for ShapeTable {
    fn default() -> Self {
        Self {
            shapes: vec![Shape::default()],
            layouts: vec![Layout::default()],
            charge: Charge::new(Kind::Shapes, size_of::<Shape>() + size_of::<Layout>()),
        }
    }
}

impl Layout {
    fn new(entries: Vec<(ObjectKey, Attributes)>) -> Self {
        let slots = entries
            .iter()
            .enumerate()
            .map(|(slot, (key, _))| (*key, slot as u32))
            .collect();
        Self { entries, slots }
    }

    /// Owned bytes of the entries and the slot map; a map entry costs its pair plus a control byte.
    fn bytes(&self) -> usize {
        self.entries.capacity() * size_of::<(ObjectKey, Attributes)>()
            + self.slots.capacity() * (size_of::<(ObjectKey, u32)>() + 1)
    }
}

impl Shape {
    fn bytes(&self) -> usize {
        self.transitions.capacity() * (size_of::<(Transition, ShapeId)>() + 1)
    }
}

impl ShapeTable {
    pub const ROOT: ShapeId = ShapeId(0);

    fn shape(&self, id: ShapeId) -> &Shape {
        &self.shapes[id.0 as usize]
    }

    pub(crate) fn lookup(&self, id: ShapeId, key: ObjectKey) -> Option<(u32, Attributes)> {
        let shape = self.shape(id);
        let layout = &self.layouts[shape.layout as usize];
        let slot = *layout.slots.get(&key)?;
        (slot < shape.len).then(|| (slot, layout.entries[slot as usize].1))
    }

    pub(crate) fn layout(&self, id: ShapeId) -> &[(ObjectKey, Attributes)] {
        let shape = self.shape(id);
        &self.layouts[shape.layout as usize].entries[..shape.len as usize]
    }

    /// Shape with `key` appended as the next slot.
    pub(crate) fn add(&mut self, from: ShapeId, key: ObjectKey, attributes: Attributes) -> ShapeId {
        let transition = Transition::Add(key, attributes);
        if let Some(target) = self.shape(from).transitions.get(&transition) {
            return *target;
        }
        let Shape { layout, len, .. } = *self.shape(from);
        let shared = &mut self.layouts[layout as usize];
        if shared.entries.len() == len as usize {
            let before = shared.bytes();
            shared.entries.push((key, attributes));
            shared.slots.insert(key, len);
            let grown = shared.bytes() - before;
            return self.push_shape(from, transition, layout, len + 1, grown);
        }
        let mut entries = self.layout(from).to_vec();
        entries.push((key, attributes));
        self.push_layout(from, transition, entries)
    }

    /// Same layout with the attributes of `key` replaced.
    pub(crate) fn reconfigure(
        &mut self,
        from: ShapeId,
        key: ObjectKey,
        attributes: Attributes,
    ) -> ShapeId {
        let transition = Transition::Reconfigure(key, attributes);
        if let Some(target) = self.shape(from).transitions.get(&transition) {
            return *target;
        }
        let mut entries = self.layout(from).to_vec();
        let entry = entries.iter_mut().find(|(k, _)| *k == key);
        entry.expect("reconfigured key is in the shape").1 = attributes;
        self.push_layout(from, transition, entries)
    }

    /// New shape over a new layout holding exactly `entries`.
    fn push_layout(
        &mut self,
        from: ShapeId,
        transition: Transition,
        entries: Vec<(ObjectKey, Attributes)>,
    ) -> ShapeId {
        let index = u32::try_from(self.layouts.len()).expect("layout table overflow");
        let len = entries.len() as u32;
        let layout = Layout::new(entries);
        let table_before = self.layouts.capacity();
        let mut added = layout.bytes();
        self.layouts.push(layout);
        added += (self.layouts.capacity() - table_before) * size_of::<Layout>();
        self.push_shape(from, transition, index, len, added)
    }

    /// Records the new shape and its transition from `from`; `added` bytes are already
    /// owned by its layout.
    fn push_shape(
        &mut self,
        from: ShapeId,
        transition: Transition,
        layout: u32,
        len: u32,
        mut added: usize,
    ) -> ShapeId {
        let id = ShapeId(u32::try_from(self.shapes.len()).expect("shape table overflow"));
        let parent_before = self.shape(from).bytes();
        let table_before = self.shapes.capacity();
        self.shapes.push(Shape {
            layout,
            len,
            transitions: HashMap::new(),
        });
        let parent = &mut self.shapes[from.0 as usize];
        parent.transitions.insert(transition, id);
        added += parent.bytes() - parent_before;
        added += (self.shapes.capacity() - table_before) * size_of::<Shape>();
        self.charge.grow(added);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: Attributes = Attributes {
        writable: true,
        enumerable: true,
        configurable: true,
    };

    fn key(n: u32) -> ObjectKey {
        ObjectKey::Index(n)
    }

    #[test]
    fn chains_share_a_layout_and_branches_copy_it() {
        let mut table = ShapeTable::default();
        let a = table.add(ShapeTable::ROOT, key(1), PLAIN);
        let ab = table.add(a, key(2), PLAIN);
        let ac = table.add(a, key(3), PLAIN);
        assert_eq!(table.shape(a).layout, table.shape(ab).layout);
        assert_ne!(table.shape(ab).layout, table.shape(ac).layout);
        assert_eq!(table.lookup(a, key(2)), None);
        assert_eq!(table.lookup(ab, key(2)), Some((1, PLAIN)));
        assert_eq!(table.lookup(ac, key(3)), Some((1, PLAIN)));
        assert_eq!(table.lookup(ac, key(2)), None);
        assert_eq!(table.layout(ac), &[(key(1), PLAIN), (key(3), PLAIN)]);
        assert_eq!(table.add(a, key(2), PLAIN), ab);
    }

    #[test]
    fn reconfigure_keeps_the_parent_layout() {
        let mut table = ShapeTable::default();
        let a = table.add(ShapeTable::ROOT, key(1), PLAIN);
        let frozen = Attributes {
            writable: false,
            ..PLAIN
        };
        let b = table.reconfigure(a, key(1), frozen);
        assert_eq!(table.lookup(a, key(1)), Some((0, PLAIN)));
        assert_eq!(table.lookup(b, key(1)), Some((0, frozen)));
    }
}

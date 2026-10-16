//! Monomorphic inline cache for one property access site: (shape id, slot offset).

use super::storage::Stored;
use super::{InternalMethods, ObjectKey, ObjectRef, ObjectResult, Realm, ShapeId};
use crate::Value;

/// Hits only on an own data property of an object whose current shape equals the
/// cached one; any transition changes the shape id, so stale entries simply miss.
/// One cache per site: use either `load` or `store`, never both.
#[derive(Debug, Default)]
pub struct InlineCache {
    entry: Option<(ShapeId, u32)>,
    misses: u32,
}

impl InlineCache {
    pub fn cached_shape(&self) -> Option<ShapeId> {
        self.entry.map(|(shape, _)| shape)
    }

    pub fn misses(&self) -> u32 {
        self.misses
    }

    /// Whether `load`/`store` on `o` would be served from the cached own slot.
    pub fn hits(&self, realm: &Realm, o: ObjectRef) -> bool {
        matches!(self.hit(realm, o), Some(Stored::Value(_)))
    }

    /// `o[key]` with `o` as receiver.
    pub fn load(&mut self, realm: &mut Realm, o: ObjectRef, key: ObjectKey) -> ObjectResult<Value> {
        if let Some(Stored::Value(value)) = self.hit(realm, o) {
            return Ok(value.clone());
        }
        self.misses += 1;
        self.entry = realm.cacheable_slot(o, key, false);
        realm.get(o, key, &Value::Object(o.raw()))
    }

    /// `o[key] = value` with `o` as receiver.
    pub fn store(
        &mut self,
        realm: &mut Realm,
        o: ObjectRef,
        key: ObjectKey,
        value: Value,
    ) -> ObjectResult<bool> {
        // The cached shape guarantees a writable slot; the slot kind is checked here.
        if let Some((_, slot)) = self.entry
            && self
                .hit(realm, o)
                .is_some_and(|stored| matches!(stored, Stored::Value(_)))
        {
            let slot = realm.object_mut(o).properties.slot_mut(slot);
            *slot.expect("cached slot exists") = Stored::Value(value);
            return Ok(true);
        }
        self.misses += 1;
        self.entry = realm.cacheable_slot(o, key, true);
        realm.set(o, key, value, &Value::Object(o.raw()))
    }

    fn hit<'a>(&self, realm: &'a Realm, o: ObjectRef) -> Option<&'a Stored> {
        let (shape, slot) = self.entry?;
        let properties = &realm.object(o).properties;
        (properties.shape_id() == Some(shape)).then(|| properties.slot(slot))?
    }
}

//! Insertion-ordered table behind Map and Set. Deleted entries leave a tombstone so live
//! iterators keep their position (ECMA-262 §24.1.3.5 iteration semantics).

use std::collections::HashMap;
use std::rc::Rc;

use velyr_engine_api::JsBigInt;
use velyr_engine_gc::external::{Charge, Kind};
use velyr_engine_gc::{GcRef, Trace, Tracer};

use crate::{JsStr, SymbolId, Value};

/// A key normalized for SameValueZero: `-0` is `+0` and every NaN is the same key.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum MapKey {
    Undefined,
    Null,
    Bool(bool),
    Number(u64),
    String(JsStr),
    Symbol(SymbolId),
    BigInt(Rc<JsBigInt>),
    Object(GcRef),
}

impl MapKey {
    pub fn new(value: &Value) -> Self {
        match value {
            Value::Undefined => MapKey::Undefined,
            Value::Null => MapKey::Null,
            Value::Bool(b) => MapKey::Bool(*b),
            Value::Number(n) if n.is_nan() => MapKey::Number(f64::NAN.to_bits()),
            Value::Number(n) if *n == 0.0 => MapKey::Number(0),
            Value::Number(n) => MapKey::Number(n.to_bits()),
            Value::String(s) => MapKey::String(s.clone()),
            Value::Symbol(s) => MapKey::Symbol(*s),
            Value::BigInt(b) => MapKey::BigInt(b.clone()),
            Value::Object(r) => MapKey::Object(*r),
        }
    }
}

/// CanonicalizeKeyedCollectionKey: `-0` is stored as `+0`.
pub(crate) fn canonical_key(value: &Value) -> Value {
    match value {
        Value::Number(n) if *n == 0.0 => Value::Number(0.0),
        other => other.clone(),
    }
}

#[derive(Clone)]
pub(crate) struct OrderedTable {
    entries: Vec<Option<(Value, Value)>>,
    index: HashMap<MapKey, usize>,
    /// Both tables by capacity, as `Kind::Storage` like object storage (they only grow).
    charge: Charge,
}

impl Default for OrderedTable {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
            charge: Charge::new(Kind::Storage, 0),
        }
    }
}

impl OrderedTable {
    pub fn get(&self, key: &Value) -> Option<&Value> {
        let position = *self.index.get(&MapKey::new(key))?;
        self.entries[position].as_ref().map(|(_, value)| value)
    }

    pub fn has(&self, key: &Value) -> bool {
        self.index.contains_key(&MapKey::new(key))
    }

    pub fn set(&mut self, key: Value, value: Value) {
        let map_key = MapKey::new(&key);
        if let Some(&position) = self.index.get(&map_key) {
            if let Some(entry) = &mut self.entries[position] {
                entry.1 = value;
            }
            return;
        }
        self.index.insert(map_key, self.entries.len());
        self.entries.push(Some((canonical_key(&key), value)));
        // A map entry costs its pair plus a control byte.
        self.charge.set(
            self.entries.capacity() * size_of::<Option<(Value, Value)>>()
                + self.index.capacity() * (size_of::<(MapKey, usize)>() + 1),
        );
    }

    pub fn delete(&mut self, key: &Value) -> bool {
        match self.index.remove(&MapKey::new(key)) {
            Some(position) => {
                self.entries[position] = None;
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.index.clear();
        for entry in &mut self.entries {
            *entry = None;
        }
    }

    pub fn size(&self) -> usize {
        self.index.len()
    }

    /// The first live entry at or after `position`, with the position after it.
    pub fn entry_from(&self, position: usize) -> Option<(usize, Value, Value)> {
        let (offset, entry) = self.entries[position.min(self.entries.len())..]
            .iter()
            .enumerate()
            .find_map(|(offset, entry)| entry.as_ref().map(|entry| (offset, entry)))?;
        Some((position + offset + 1, entry.0.clone(), entry.1.clone()))
    }
}

impl Trace for OrderedTable {
    fn trace(&self, tracer: &mut Tracer) {
        for (key, value) in self.entries.iter().flatten() {
            key.trace(tracer);
            value.trace(tracer);
        }
    }
}

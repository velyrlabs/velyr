//! Weak cells understood by the collector: WeakRef, WeakMap (ephemerons) and
//! FinalizationRegistry.

use std::collections::HashMap;

use crate::{GcRef, Liveness, Trace, Tracer};

/// Holds its target without keeping it alive; cleared once the target is collected.
#[derive(Debug)]
pub struct WeakRef {
    target: Option<GcRef>,
}

impl WeakRef {
    pub fn new(target: GcRef) -> Self {
        Self {
            target: Some(target),
        }
    }

    pub fn deref(&self) -> Option<GcRef> {
        self.target
    }
}

impl Trace for WeakRef {
    fn trace(&self, _: &mut Tracer) {}

    fn clear_dead_refs(&mut self, liveness: &Liveness) -> bool {
        self.target = self.target.filter(|target| liveness.is_live(*target));
        false
    }
}

/// Ephemeron table: a value is kept alive only while its key is reachable elsewhere.
#[derive(Debug)]
pub struct WeakMap<V> {
    entries: HashMap<GcRef, V>,
}

impl<V> Default for WeakMap<V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<V> WeakMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: GcRef, value: V) -> Option<V> {
        self.entries.insert(key, value)
    }

    pub fn get(&self, key: GcRef) -> Option<&V> {
        self.entries.get(&key)
    }

    pub fn remove(&mut self, key: GcRef) -> Option<V> {
        self.entries.remove(&key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<V: Trace> Trace for WeakMap<V> {
    fn trace(&self, tracer: &mut Tracer) {
        for (key, value) in &self.entries {
            tracer.ephemeron(*key, value);
        }
    }

    fn clear_dead_refs(&mut self, liveness: &Liveness) -> bool {
        self.entries.retain(|key, _| liveness.is_live(*key));
        false
    }
}

/// Targets are weak, held values strong. When a target dies its held value moves to the
/// cleanup list and the registry is queued on the heap; callbacks are run by the caller.
#[derive(Debug)]
pub struct FinalizationRegistry<V> {
    registrations: Vec<(GcRef, V)>,
    cleanups: Vec<V>,
}

impl<V> Default for FinalizationRegistry<V> {
    fn default() -> Self {
        Self {
            registrations: Vec::new(),
            cleanups: Vec::new(),
        }
    }
}

impl<V> FinalizationRegistry<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, target: GcRef, held: V) {
        self.registrations.push((target, held));
    }

    /// Held values whose targets were collected, in collection order.
    pub fn take_cleanups(&mut self) -> Vec<V> {
        std::mem::take(&mut self.cleanups)
    }
}

impl<V: Trace> Trace for FinalizationRegistry<V> {
    fn trace(&self, tracer: &mut Tracer) {
        self.registrations
            .iter()
            .for_each(|(_, held)| held.trace(tracer));
        self.cleanups.trace(tracer);
    }

    fn clear_dead_refs(&mut self, liveness: &Liveness) -> bool {
        let pending_before = self.cleanups.len();
        let dead = self
            .registrations
            .extract_if(.., |(target, _)| !liveness.is_live(*target));
        self.cleanups.extend(dead.map(|(_, held)| held));
        self.cleanups.len() > pending_before
    }
}

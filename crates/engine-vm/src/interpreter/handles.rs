//! Embedder handles: generation-checked slots that keep objects alive (they are part of
//! the interpreter's root set) until released one by one or by leaving their scope.
//! Same contract as engine-v8's `HandleTable`.

use velyr_engine_api::Handle;
use velyr_engine_gc::{Trace, Tracer};

use super::Interpreter;
use crate::Value;

struct Slot {
    generation: u32,
    value: Option<Value>,
}

#[derive(Default)]
pub(crate) struct HandleTable {
    slots: Vec<Slot>,
    free: Vec<u32>,
    scopes: Vec<Vec<Handle>>,
}

impl HandleTable {
    fn alloc(&mut self, value: Value) -> Handle {
        let id = match self.free.pop() {
            Some(id) => id,
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    value: None,
                });
                u32::try_from(self.slots.len() - 1).expect("handle table overflow")
            }
        };
        let slot = &mut self.slots[id as usize];
        slot.value = Some(value);
        let handle = Handle {
            id,
            generation: slot.generation,
        };
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(handle);
        }
        handle
    }

    fn get(&self, handle: Handle) -> Option<&Value> {
        let slot = self.slots.get(handle.id as usize)?;
        (slot.generation == handle.generation)
            .then_some(slot.value.as_ref())
            .flatten()
    }

    /// Stale or already released handles are ignored.
    fn release(&mut self, handle: Handle) {
        let Some(slot) = self.slots.get_mut(handle.id as usize) else {
            return;
        };
        if slot.generation != handle.generation || slot.value.is_none() {
            return;
        }
        slot.value = None;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(handle.id);
    }
}

impl Trace for HandleTable {
    fn trace(&self, tracer: &mut Tracer) {
        for slot in &self.slots {
            slot.value.trace(tracer);
        }
    }
}

impl Interpreter {
    /// Roots `value` (an object) behind a new handle; `None` for primitives.
    pub fn create_handle(&mut self, value: &Value) -> Option<Handle> {
        matches!(value, Value::Object(_)).then(|| self.state.handles.alloc(value.clone()))
    }

    /// The object behind a live handle.
    pub fn resolve_handle(&self, handle: Handle) -> Option<Value> {
        self.state.handles.get(handle).cloned()
    }

    pub fn release_handle(&mut self, handle: Handle) {
        self.state.handles.release(handle);
    }

    /// Handles created until the matching `pop_handle_scope` are released by it.
    pub fn push_handle_scope(&mut self) {
        self.state.handles.scopes.push(Vec::new());
    }

    pub fn pop_handle_scope(&mut self) {
        for handle in self.state.handles.scopes.pop().unwrap_or_default() {
            self.state.handles.release(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_slots_are_reused_with_a_new_generation() {
        let mut table = HandleTable::default();
        let first = table.alloc(Value::Null);
        table.release(first);
        table.release(first);
        let second = table.alloc(Value::Bool(true));
        assert_eq!(second.id, first.id);
        assert_ne!(second.generation, first.generation);
        assert!(table.get(first).is_none());
        assert!(table.get(second).is_some());
    }
}

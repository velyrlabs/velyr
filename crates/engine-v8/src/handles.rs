//! Table of `v8::Global` values addressed by generation-checked [`Handle`]s.

use velyr_engine_api::Handle;

struct Slot {
    generation: u32,
    value: Option<v8::Global<v8::Value>>,
}

#[derive(Default)]
pub(crate) struct HandleTable {
    slots: Vec<Slot>,
    free: Vec<u32>,
    scopes: Vec<Vec<Handle>>,
}

impl HandleTable {
    pub(crate) fn alloc(&mut self, value: v8::Global<v8::Value>) -> Handle {
        let handle = match self.free.pop() {
            Some(id) => {
                let slot = &mut self.slots[id as usize];
                slot.value = Some(value);
                Handle {
                    id,
                    generation: slot.generation,
                }
            }
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    value: Some(value),
                });
                Handle {
                    id: u32::try_from(self.slots.len() - 1).expect("handle table overflow"),
                    generation: 0,
                }
            }
        };
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(handle);
        }
        handle
    }

    pub(crate) fn get(&self, handle: Handle) -> Option<&v8::Global<v8::Value>> {
        let slot = self.slots.get(handle.id as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_ref()
    }

    /// Stale or already released handles are ignored.
    pub(crate) fn release(&mut self, handle: Handle) {
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

    pub(crate) fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    pub(crate) fn pop_scope(&mut self) {
        for handle in self.scopes.pop().unwrap_or_default() {
            self.release(handle);
        }
    }
}

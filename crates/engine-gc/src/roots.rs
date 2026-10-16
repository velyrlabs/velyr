//! Host roots for engine-api: scoped handles (`HandleScope`) and persistent ones (`Global`).

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::{Gc, GcRef, Heap};

/// Roots added with `root_local` live until the scope is dropped. Derefs to the heap,
/// so scopes nest: `HandleScope::new(&mut outer)`.
pub struct HandleScope<'h> {
    heap: &'h mut Heap,
    base: usize,
}

impl<'h> HandleScope<'h> {
    pub fn new(heap: &'h mut Heap) -> Self {
        let base = heap.scoped_roots.len();
        Self { heap, base }
    }

    pub fn root_local(&mut self, target: GcRef) {
        self.heap.scoped_roots.push(target);
    }
}

impl Deref for HandleScope<'_> {
    type Target = Heap;

    fn deref(&self) -> &Heap {
        self.heap
    }
}

impl DerefMut for HandleScope<'_> {
    fn deref_mut(&mut self) -> &mut Heap {
        self.heap
    }
}

impl Drop for HandleScope<'_> {
    fn drop(&mut self) {
        self.heap.scoped_roots.truncate(self.base);
    }
}

/// Persistent root owned by the host; released explicitly with `Heap::release_global`.
/// Only valid on the heap that created it.
#[derive(Debug)]
pub struct Global<T> {
    slot: usize,
    marker: PhantomData<fn() -> T>,
}

impl Heap {
    pub fn make_global<T>(&mut self, target: Gc<T>) -> Global<T> {
        let slot = match self.free_globals.pop() {
            Some(slot) => {
                self.globals[slot] = Some(target.raw());
                slot
            }
            None => {
                self.globals.push(Some(target.raw()));
                self.globals.len() - 1
            }
        };
        Global {
            slot,
            marker: PhantomData,
        }
    }

    pub fn global_get<T>(&self, global: &Global<T>) -> Gc<T> {
        let raw = self.globals[global.slot].expect("live Global points at an occupied slot");
        Gc::from_raw(raw)
    }

    pub fn release_global<T>(&mut self, global: Global<T>) {
        self.globals[global.slot] = None;
        self.free_globals.push(global.slot);
    }
}

//! Heap and collector for the native engine (ADR-009, Track C, VELYR-101).
//!
//! Per-isolate heap of indexed slots; stop-the-world, non-moving mark-sweep. A handle is a
//! slot index plus generation, so it stays valid for the cell's whole life and a reused
//! slot never resurrects a stale handle.

mod heap;
mod roots;
mod trace;
mod weak;

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

pub use heap::{GcStats, Heap, HeapConfig};
pub use roots::{Global, HandleScope};
pub use trace::{Liveness, Trace, Tracer};
pub use weak::{FinalizationRegistry, WeakMap, WeakRef};

/// Opaque, copyable reference to a heap cell: slot index plus a generation that
/// detects reuse of a freed slot. 8 bytes, so `Value` stays at 16.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GcRef {
    index: u32,
    generation: u32,
}

impl GcRef {
    pub const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    pub const fn index(self) -> u32 {
        self.index
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

/// Typed handle to a heap cell holding a `T`; erases to `GcRef` with `raw`.
pub struct Gc<T> {
    raw: GcRef,
    marker: PhantomData<fn() -> T>,
}

impl<T> Gc<T> {
    /// The caller asserts the cell holds a `T`; `Heap::get` re-checks the type.
    pub const fn from_raw(raw: GcRef) -> Self {
        Self {
            raw,
            marker: PhantomData,
        }
    }

    pub const fn raw(self) -> GcRef {
        self.raw
    }
}

impl<T> Clone for Gc<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Gc<T> {}

impl<T> PartialEq for Gc<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T> Eq for Gc<T> {}

impl<T> Hash for Gc<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl<T> fmt::Debug for Gc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Gc").field(&self.raw).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_is_eight_bytes_and_compares_by_slot_and_generation() {
        assert_eq!(size_of::<GcRef>(), 8);
        assert_eq!(GcRef::new(3, 1), GcRef::new(3, 1));
        assert_ne!(GcRef::new(3, 1), GcRef::new(3, 2));
        assert_eq!(GcRef::new(3, 1).index(), 3);
        assert_eq!(GcRef::new(3, 1).generation(), 1);
    }
}

//! Bytes the engine holds outside heap cells, by kind. Strings, object storage, shapes,
//! the atom table and the engine side tables count against the hard limit of the heap;
//! bytecode and source text are only attributed, for `GcStats::breakdown` and the memory
//! metrics.
//!
//! Each heap owns one counter per kind, and charges go to the heap that last entered its
//! thread (`enter`), so charging is a plain add with no lock. Isolates are `!Send`, so a
//! value is charged and released on the same thread.
//! shortcut: a value charged by one isolate and dropped while another one is entered is
//! released from the wrong counters (saturating); exact attribution needs a heap pointer
//! in every string.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Strings,
    Storage,
    Code,
    Source,
    Shapes,
    Interner,
    /// Engine side tables: weak maps, closure records, per-code info.
    Tables,
}

const KINDS: usize = 7;

/// Per-kind byte counters of one heap.
pub type Counters = Rc<[Cell<usize>; KINDS]>;

thread_local! {
    static CURRENT: RefCell<Counters> = RefCell::default();
}

/// Sends this thread's later charges and releases to `counters`.
#[inline]
pub fn enter(counters: &Counters) {
    CURRENT.with(|current| {
        if !Rc::ptr_eq(&current.borrow(), counters) {
            *current.borrow_mut() = counters.clone();
        }
    });
}

#[inline]
fn with_current<R>(f: impl FnOnce(&[Cell<usize>; KINDS]) -> R) -> R {
    CURRENT.with(|current| f(&current.borrow()))
}

/// Per-kind bytes of one heap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Breakdown {
    pub strings: usize,
    pub storage: usize,
    pub code: usize,
    pub source: usize,
    pub shapes: usize,
    pub interner: usize,
    pub tables: usize,
}

#[inline]
pub fn charge(kind: Kind, bytes: usize) {
    with_current(|all| {
        let total = &all[kind as usize];
        total.set(total.get() + bytes);
    });
}

#[inline]
pub fn release(kind: Kind, bytes: usize) {
    with_current(|all| {
        let total = &all[kind as usize];
        total.set(total.get().saturating_sub(bytes));
    });
}

/// Charges or releases the difference between what a value held and what it holds now.
#[inline]
pub fn adjust(kind: Kind, charged: &mut usize, now: usize) {
    if now > *charged {
        charge(kind, now - *charged);
    } else {
        release(kind, *charged - now);
    }
    *charged = now;
}

/// Bytes charged to one kind for as long as the owner lives: released on drop, charged
/// again by a clone.
#[derive(Debug)]
pub struct Charge {
    kind: Kind,
    bytes: usize,
}

impl Charge {
    pub fn new(kind: Kind, bytes: usize) -> Self {
        charge(kind, bytes);
        Self { kind, bytes }
    }

    /// Moves the charge to `bytes`.
    pub fn set(&mut self, bytes: usize) {
        adjust(self.kind, &mut self.bytes, bytes);
    }

    /// Adds `bytes` to the charge.
    pub fn grow(&mut self, bytes: usize) {
        charge(self.kind, bytes);
        self.bytes += bytes;
    }
}

impl Clone for Charge {
    fn clone(&self) -> Self {
        Self::new(self.kind, self.bytes)
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        release(self.kind, self.bytes);
    }
}

/// Bytes in `counters` that count against the heap limit: every kind but code and source.
#[inline]
pub fn bytes(counters: &Counters) -> usize {
    let all = counters;
    all[Kind::Strings as usize].get()
        + all[Kind::Storage as usize].get()
        + all[Kind::Shapes as usize].get()
        + all[Kind::Interner as usize].get()
        + all[Kind::Tables as usize].get()
}

pub fn breakdown(counters: &Counters) -> Breakdown {
    let all = counters;
    Breakdown {
        strings: all[Kind::Strings as usize].get(),
        storage: all[Kind::Storage as usize].get(),
        code: all[Kind::Code as usize].get(),
        source: all[Kind::Source as usize].get(),
        shapes: all[Kind::Shapes as usize].get(),
        interner: all[Kind::Interner as usize].get(),
        tables: all[Kind::Tables as usize].get(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static OWN: Counters = Counters::default();
    }

    /// Enters this thread's test counters and reads them.
    fn bytes() -> usize {
        OWN.with(|own| {
            enter(own);
            super::bytes(own)
        })
    }

    fn breakdown() -> Breakdown {
        OWN.with(|own| {
            enter(own);
            super::breakdown(own)
        })
    }

    #[test]
    fn charges_go_to_the_entered_counters() {
        let (first, second) = (Counters::default(), Counters::default());
        enter(&first);
        charge(Kind::Strings, 10);
        enter(&second);
        charge(Kind::Shapes, 4);
        assert_eq!((super::bytes(&first), super::bytes(&second)), (10, 4));
        release(Kind::Shapes, 4);
        enter(&first);
        release(Kind::Strings, 10);
        assert_eq!((super::bytes(&first), super::bytes(&second)), (0, 0));
    }

    #[test]
    fn charges_and_releases_balance() {
        let start = bytes();
        let mut held = 0;
        adjust(Kind::Storage, &mut held, 100);
        charge(Kind::Strings, 20);
        assert_eq!(bytes(), start + 120);
        adjust(Kind::Storage, &mut held, 40);
        release(Kind::Strings, 20);
        adjust(Kind::Storage, &mut held, 0);
        assert_eq!(bytes(), start);
    }

    #[test]
    fn attributed_kinds_do_not_count_against_the_limit() {
        let start = (bytes(), breakdown());
        charge(Kind::Code, 64);
        charge(Kind::Source, 8);
        assert_eq!(bytes(), start.0);
        assert_eq!(breakdown().code, start.1.code + 64);
        assert_eq!(breakdown().source, start.1.source + 8);
        release(Kind::Code, 64);
        release(Kind::Source, 8);
        assert_eq!(breakdown(), start.1);
    }

    #[test]
    fn a_charge_follows_its_owner_and_its_clones() {
        let start = breakdown().shapes;
        let mut held = Charge::new(Kind::Shapes, 10);
        let copy = held.clone();
        assert_eq!(breakdown().shapes, start + 20);
        held.set(30);
        assert_eq!(breakdown().shapes, start + 40);
        drop(copy);
        drop(held);
        assert_eq!(breakdown().shapes, start);
    }
}

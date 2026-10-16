//! Tracing: the `Trace` trait and the marker that walks the object graph.

use std::any::Any;

use crate::{Gc, GcRef};

/// Implemented by everything the collector can walk: heap cells and root sets.
pub trait Trace {
    /// Reports every strong reference via `tracer.mark` (or `tracer.ephemeron`).
    fn trace(&self, tracer: &mut Tracer);

    /// Runs on live cells after marking so weak references to dead cells can be dropped.
    /// Returns true when the cell queued new finalization work.
    fn clear_dead_refs(&mut self, _liveness: &Liveness) -> bool {
        false
    }

    /// Bytes a heap cell owns outside itself that it never resizes (fixed buffers);
    /// charged with the cell. Storage that grows is charged through `external` instead.
    fn owned_bytes(&self) -> usize {
        0
    }
}

/// A heap cell: traceable and downcastable to its concrete type.
pub(crate) trait Cell: Trace + Any {}

impl<T: Trace + Any> Cell for T {}

pub(crate) type CellSlot = Option<Box<dyn Cell>>;

/// Mark-phase state. Mark bits live outside the cells, so the graph is only read.
pub struct Tracer<'a> {
    generations: &'a [u32],
    cells: &'a [CellSlot],
    marks: &'a mut [bool],
    worklist: Vec<u32>,
    ephemeron_cells: Vec<u32>,
    has_pending_ephemeron: bool,
    marked_count: usize,
}

impl<'a> Tracer<'a> {
    pub(crate) fn new(
        generations: &'a [u32],
        cells: &'a [CellSlot],
        marks: &'a mut [bool],
    ) -> Self {
        Self {
            generations,
            cells,
            marks,
            worklist: Vec::new(),
            ephemeron_cells: Vec::new(),
            has_pending_ephemeron: false,
            marked_count: 0,
        }
    }

    /// Marks `target` reachable. Stale or foreign handles are ignored.
    pub fn mark(&mut self, target: GcRef) {
        if !self.is_valid(target) || self.marks[target.index() as usize] {
            return;
        }
        self.marks[target.index() as usize] = true;
        self.marked_count += 1;
        self.worklist.push(target.index());
    }

    /// Ephemeron entry: `value` is traced only once `key` is marked. Only cells in the
    /// heap are revisited at the fixpoint; ephemerons reported by root sets are ignored.
    pub fn ephemeron(&mut self, key: GcRef, value: &dyn Trace) {
        if self.is_marked(key) {
            value.trace(self);
        } else {
            self.has_pending_ephemeron = true;
        }
    }

    fn is_valid(&self, target: GcRef) -> bool {
        let index = target.index() as usize;
        self.generations.get(index) == Some(&target.generation()) && self.cells[index].is_some()
    }

    fn is_marked(&self, target: GcRef) -> bool {
        self.is_valid(target) && self.marks[target.index() as usize]
    }

    /// Drains the worklist, then re-traces ephemeron tables until no new cell is marked.
    pub(crate) fn run_to_fixpoint(&mut self) {
        loop {
            while let Some(index) = self.worklist.pop() {
                self.trace_cell(index);
            }
            let marked_before = self.marked_count;
            for index in std::mem::take(&mut self.ephemeron_cells) {
                self.trace_cell(index);
            }
            if self.marked_count == marked_before {
                return;
            }
        }
    }

    fn trace_cell(&mut self, index: u32) {
        let cells = self.cells;
        let Some(cell) = &cells[index as usize] else {
            return;
        };
        self.has_pending_ephemeron = false;
        cell.trace(self);
        if self.has_pending_ephemeron {
            self.ephemeron_cells.push(index);
        }
    }
}

/// Post-mark view of which cells survived the current collection.
pub struct Liveness<'a> {
    pub(crate) generations: &'a [u32],
    pub(crate) marks: &'a [bool],
}

impl Liveness<'_> {
    pub fn is_live(&self, target: GcRef) -> bool {
        let index = target.index() as usize;
        self.generations.get(index) == Some(&target.generation())
            && self.marks.get(index).copied().unwrap_or(false)
    }
}

impl Trace for () {
    fn trace(&self, _: &mut Tracer) {}
}

impl Trace for GcRef {
    fn trace(&self, tracer: &mut Tracer) {
        tracer.mark(*self);
    }
}

impl<T> Trace for Gc<T> {
    fn trace(&self, tracer: &mut Tracer) {
        tracer.mark(self.raw());
    }
}

impl<T: Trace> Trace for Option<T> {
    fn trace(&self, tracer: &mut Tracer) {
        if let Some(value) = self {
            value.trace(tracer);
        }
    }
}

impl<T: Trace> Trace for [T] {
    fn trace(&self, tracer: &mut Tracer) {
        self.iter().for_each(|value| value.trace(tracer));
    }
}

impl<T: Trace> Trace for Vec<T> {
    fn trace(&self, tracer: &mut Tracer) {
        self.as_slice().trace(tracer);
    }
}

impl<T: Trace + ?Sized> Trace for &T {
    fn trace(&self, tracer: &mut Tracer) {
        (**self).trace(tracer);
    }
}

impl<A: Trace, B: Trace> Trace for (A, B) {
    fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
    }
}

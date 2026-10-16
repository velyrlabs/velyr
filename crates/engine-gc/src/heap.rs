//! Per-isolate heap: indexed slots, stop-the-world non-moving mark-sweep.

use std::any::Any;
use std::time::{Duration, Instant};

use velyr_runtime_errors::VelyrError;

use crate::external;
use crate::trace::{Cell, CellSlot};
use crate::{Gc, GcRef, Liveness, Trace, Tracer};

/// Bookkeeping charged per cell on top of the object itself (slot, generation, mark bit).
const CELL_OVERHEAD_BYTES: usize = size_of::<CellSlot>() + size_of::<u32>() + size_of::<bool>();
const DEFAULT_GC_THRESHOLD_BYTES: usize = 1 << 20;
const QUOTA_CLASS: &str = "heap";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapConfig {
    /// Bytes allocated since the last collection that trigger the next one.
    pub gc_threshold_bytes: usize,
    /// Isolate heap budget (VELYR-29); allocations past it fail with `QuotaExceeded`.
    pub hard_limit_bytes: Option<usize>,
}

impl Default for HeapConfig {
    fn default() -> Self {
        Self {
            gc_threshold_bytes: DEFAULT_GC_THRESHOLD_BYTES,
            hard_limit_bytes: None,
        }
    }
}

/// Collector metrics; engine-native maps them onto the engine-api `HeapStats`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcStats {
    pub collections: u64,
    pub last_pause: Duration,
    pub total_pause: Duration,
    /// Bytes held by allocated cells; exact right after a collection.
    pub live_bytes: usize,
    pub live_cells: usize,
    /// Bytes held outside the cells by this heap that count against the limit (see
    /// `external`).
    pub external_bytes: usize,
    /// Every external kind of this heap, including those outside the limit.
    pub breakdown: external::Breakdown,
    /// The heap's own slot tables (cells, generations, marks, free list), by capacity.
    pub table_bytes: usize,
}

pub struct Heap {
    config: HeapConfig,
    /// This heap's external bytes; entered by `reserve` (every safepoint), `collect` and
    /// the embedder's entry points, not by `alloc`, which is too hot for the check.
    counters: external::Counters,
    /// How far past the hard limit the heap may grow before the next collection: an
    /// eighth of what survived the last one, so a heap that stays near the limit does
    /// not collect on every allocation.
    limit_slack: usize,
    generations: Vec<u32>,
    cells: Vec<CellSlot>,
    marks: Vec<bool>,
    free_slots: Vec<u32>,
    allocated_bytes: usize,
    live_cells: usize,
    bytes_since_gc: usize,
    pub(crate) scoped_roots: Vec<GcRef>,
    pub(crate) globals: Vec<Option<GcRef>>,
    pub(crate) free_globals: Vec<usize>,
    finalization_queue: Vec<GcRef>,
    collections: u64,
    last_pause: Duration,
    total_pause: Duration,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new(HeapConfig::default())
    }
}

impl Drop for Heap {
    /// The cells and the realm parts dropped after the heap release into its counters,
    /// not into another isolate's.
    fn drop(&mut self) {
        external::enter(&self.counters);
    }
}

impl Heap {
    /// A heap with its own external counters, entered on this thread right away so the
    /// realm built around it charges to it.
    pub fn new(config: HeapConfig) -> Self {
        let counters = external::Counters::default();
        external::enter(&counters);
        Self {
            config,
            counters,
            limit_slack: 0,
            generations: Vec::new(),
            cells: Vec::new(),
            marks: Vec::new(),
            free_slots: Vec::new(),
            allocated_bytes: 0,
            live_cells: 0,
            bytes_since_gc: 0,
            scoped_roots: Vec::new(),
            globals: Vec::new(),
            free_globals: Vec::new(),
            finalization_queue: Vec::new(),
            collections: 0,
            last_pause: Duration::ZERO,
            total_pause: Duration::ZERO,
        }
    }

    /// Sends this thread's external charges to this heap until another heap enters.
    #[inline]
    pub fn enter(&self) {
        external::enter(&self.counters);
    }

    /// Allocates `value`, collecting first when the threshold or hard limit is hit.
    /// `roots` is the caller's root set (VM stack, registers); refs inside `value` are
    /// kept alive too. Fails with `QuotaExceeded` if the heap is still over budget.
    pub fn alloc<T: Trace + 'static>(
        &mut self,
        value: T,
        roots: &dyn Trace,
    ) -> Result<Gc<T>, VelyrError> {
        let size = size_of::<T>() + CELL_OVERHEAD_BYTES + value.owned_bytes();
        if self.bytes_since_gc + size > self.config.gc_threshold_bytes
            || self.exceeds_hard_limit(size, self.limit_slack)
        {
            self.collect(&(roots, &value));
            if self.exceeds_hard_limit(size, 0) {
                return Err(VelyrError::quota_exceeded(QUOTA_CLASS));
            }
        }
        let raw = self.insert(Box::new(value))?;
        self.allocated_bytes += size;
        self.bytes_since_gc += size;
        self.live_cells += 1;
        Ok(Gc::from_raw(raw))
    }

    /// Replaces the hard limit; later allocations past it fail with `QuotaExceeded`.
    pub fn set_hard_limit(&mut self, limit: Option<usize>) {
        self.config.hard_limit_bytes = limit;
    }

    pub fn hard_limit(&self) -> Option<usize> {
        self.config.hard_limit_bytes
    }

    /// Makes room for `bytes` more outside the cells (0 checks the current total):
    /// collects when cells plus external bytes would pass the hard limit (plus the slack
    /// left by the last collection), then fails with `QuotaExceeded` if they would still
    /// pass the limit itself.
    #[inline]
    pub fn reserve(&mut self, bytes: usize, roots: &dyn Trace) -> Result<(), VelyrError> {
        self.enter();
        if !self.exceeds_hard_limit(bytes, self.limit_slack) {
            return Ok(());
        }
        self.collect(roots);
        match self.exceeds_hard_limit(bytes, 0) {
            true => Err(VelyrError::quota_exceeded(QUOTA_CLASS)),
            false => Ok(()),
        }
    }

    #[inline]
    fn exceeds_hard_limit(&self, size: usize, slack: usize) -> bool {
        self.config.hard_limit_bytes.is_some_and(|limit| {
            self.allocated_bytes
                .saturating_add(external::bytes(&self.counters))
                .saturating_add(size)
                > limit.saturating_add(slack)
        })
    }

    fn insert(&mut self, cell: Box<dyn Cell>) -> Result<GcRef, VelyrError> {
        if let Some(index) = self.free_slots.pop() {
            self.cells[index as usize] = Some(cell);
            return Ok(GcRef::new(index, self.generations[index as usize]));
        }
        let index =
            u32::try_from(self.cells.len()).map_err(|_| VelyrError::quota_exceeded(QUOTA_CLASS))?;
        self.cells.push(Some(cell));
        self.generations.push(0);
        Ok(GcRef::new(index, 0))
    }

    /// Full stop-the-world collection.
    pub fn collect(&mut self, roots: &dyn Trace) {
        self.enter();
        let started = Instant::now();
        self.mark(roots);
        self.clear_dead_refs();
        self.sweep();
        self.limit_slack = (self.allocated_bytes + external::bytes(&self.counters)) / 8;
        self.last_pause = started.elapsed();
        self.total_pause += self.last_pause;
        self.collections += 1;
    }

    fn mark(&mut self, roots: &dyn Trace) {
        self.marks.clear();
        self.marks.resize(self.cells.len(), false);
        let mut tracer = Tracer::new(&self.generations, &self.cells, &mut self.marks);
        roots.trace(&mut tracer);
        self.scoped_roots.trace(&mut tracer);
        self.globals.trace(&mut tracer);
        self.finalization_queue.trace(&mut tracer);
        tracer.run_to_fixpoint();
    }

    fn clear_dead_refs(&mut self) {
        let liveness = Liveness {
            generations: &self.generations,
            marks: &self.marks,
        };
        for (index, slot) in self.cells.iter_mut().enumerate() {
            let Some(cell) = slot.as_mut().filter(|_| self.marks[index]) else {
                continue;
            };
            let registry = GcRef::new(index as u32, self.generations[index]);
            if cell.clear_dead_refs(&liveness) && !self.finalization_queue.contains(&registry) {
                self.finalization_queue.push(registry);
            }
        }
    }

    fn sweep(&mut self) {
        for (index, slot) in self.cells.iter_mut().enumerate() {
            if self.marks[index] {
                continue;
            }
            let Some(cell) = slot.take() else {
                continue;
            };
            self.allocated_bytes -= size_of_val(&*cell) + CELL_OVERHEAD_BYTES + cell.owned_bytes();
            self.live_cells -= 1;
            let generation = &mut self.generations[index];
            // A slot whose generation would wrap is retired so stale handles never alias.
            if *generation < u32::MAX {
                *generation += 1;
                self.free_slots.push(index as u32);
            }
        }
        self.bytes_since_gc = 0;
    }

    pub fn is_alive(&self, target: GcRef) -> bool {
        self.cell(target).is_some()
    }

    fn cell(&self, target: GcRef) -> Option<&dyn Cell> {
        let index = target.index() as usize;
        if self.generations.get(index) != Some(&target.generation()) {
            return None;
        }
        self.cells[index].as_deref()
    }

    pub fn get<T: 'static>(&self, target: Gc<T>) -> Option<&T> {
        let any: &dyn Any = self.cell(target.raw())?;
        any.downcast_ref()
    }

    pub fn get_mut<T: 'static>(&mut self, target: Gc<T>) -> Option<&mut T> {
        let index = target.raw().index() as usize;
        if self.generations.get(index) != Some(&target.raw().generation()) {
            return None;
        }
        let any: &mut dyn Any = self.cells[index].as_deref_mut()?;
        any.downcast_mut()
    }

    /// Registries with pending cleanups; they stay rooted until taken. Drain each with
    /// `FinalizationRegistry::take_cleanups`.
    pub fn take_finalization_queue(&mut self) -> Vec<GcRef> {
        std::mem::take(&mut self.finalization_queue)
    }

    pub fn stats(&self) -> GcStats {
        GcStats {
            collections: self.collections,
            last_pause: self.last_pause,
            total_pause: self.total_pause,
            live_bytes: self.allocated_bytes,
            live_cells: self.live_cells,
            external_bytes: external::bytes(&self.counters),
            breakdown: external::breakdown(&self.counters),
            table_bytes: self.cells.capacity() * size_of::<CellSlot>()
                + self.generations.capacity() * size_of::<u32>()
                + self.marks.capacity() * size_of::<bool>()
                + self.free_slots.capacity() * size_of::<u32>(),
        }
    }
}

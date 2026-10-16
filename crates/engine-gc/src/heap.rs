//! Per-isolate heap: indexed slots, stop-the-world non-moving mark-sweep.

use std::any::Any;
use std::time::{Duration, Instant};

use velyr_runtime_errors::VelyrError;

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
}

#[derive(Default)]
pub struct Heap {
    config: HeapConfig,
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

impl Heap {
    pub fn new(config: HeapConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    /// Allocates `value`, collecting first when the threshold or hard limit is hit.
    /// `roots` is the caller's root set (VM stack, registers); refs inside `value` are
    /// kept alive too. Fails with `QuotaExceeded` if the heap is still over budget.
    pub fn alloc<T: Trace + 'static>(
        &mut self,
        value: T,
        roots: &dyn Trace,
    ) -> Result<Gc<T>, VelyrError> {
        let size = size_of::<T>() + CELL_OVERHEAD_BYTES;
        if self.bytes_since_gc + size > self.config.gc_threshold_bytes
            || self.exceeds_hard_limit(size)
        {
            self.collect(&(roots, &value));
        }
        if self.exceeds_hard_limit(size) {
            return Err(VelyrError::quota_exceeded(QUOTA_CLASS));
        }
        let raw = self.insert(Box::new(value))?;
        self.allocated_bytes += size;
        self.bytes_since_gc += size;
        self.live_cells += 1;
        Ok(Gc::from_raw(raw))
    }

    fn exceeds_hard_limit(&self, size: usize) -> bool {
        self.config
            .hard_limit_bytes
            .is_some_and(|limit| self.allocated_bytes + size > limit)
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
        let started = Instant::now();
        self.mark(roots);
        self.clear_dead_refs();
        self.sweep();
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
            self.allocated_bytes -= size_of_val(&*cell) + CELL_OVERHEAD_BYTES;
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
        }
    }
}

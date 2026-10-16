//! Backing stores of ArrayBuffer and SharedArrayBuffer. They live outside the GC heap
//! (an `Rc` block for ArrayBuffer, an `Arc` block behind a mutex for SharedArrayBuffer) and
//! each byte is charged to the isolate's `Budget`, so `heap_limit_bytes` bounds them.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::ObjectRef;

/// Bytes of backing stores alive in an isolate, and the most it may hold.
pub(crate) struct Budget {
    used: AtomicUsize,
    limit: AtomicUsize,
}

impl Budget {
    pub(crate) fn unlimited() -> Arc<Self> {
        Arc::new(Self {
            used: AtomicUsize::new(0),
            limit: AtomicUsize::new(usize::MAX),
        })
    }

    pub(crate) fn set_limit(&self, limit: usize) {
        self.limit.store(limit, Ordering::Relaxed);
    }
}

/// A share of a `Budget`, given back on drop.
struct Reservation {
    budget: Arc<Budget>,
    bytes: usize,
}

impl Reservation {
    /// Changes the share to `bytes`; false (and unchanged) when that passes the limit.
    fn resize(&mut self, bytes: usize) -> bool {
        if bytes <= self.bytes {
            self.budget
                .used
                .fetch_sub(self.bytes - bytes, Ordering::SeqCst);
            self.bytes = bytes;
            return true;
        }
        let extra = bytes - self.bytes;
        let limit = self.budget.limit.load(Ordering::Relaxed);
        let reserved = self
            .budget
            .used
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(extra).filter(|&total| total <= limit)
            });
        if reserved.is_ok() {
            self.bytes = bytes;
        }
        reserved.is_ok()
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

/// Zero-initialized bytes paid for by a budget.
pub(crate) struct Block {
    pub bytes: Vec<u8>,
    reservation: Reservation,
}

impl Block {
    /// `None` past the budget or when the allocation fails.
    pub(crate) fn new(budget: &Arc<Budget>, len: usize) -> Option<Self> {
        let mut block = Self {
            bytes: Vec::new(),
            reservation: Reservation {
                budget: budget.clone(),
                bytes: 0,
            },
        };
        block.resize(len).then_some(block)
    }

    /// Grows (zero-filling) or shrinks to `len`; false and unchanged on failure.
    pub(crate) fn resize(&mut self, len: usize) -> bool {
        let current = self.bytes.len();
        if !self.reservation.resize(len) {
            return false;
        }
        if len > current && self.bytes.try_reserve_exact(len - current).is_err() {
            self.reservation.resize(current);
            return false;
        }
        self.bytes.resize(len, 0);
        if len < current {
            self.bytes.shrink_to_fit();
        }
        true
    }
}

/// `[[ArrayBufferData]]` of an ArrayBuffer; `block` is `None` once detached.
pub(crate) struct LocalBuffer {
    pub block: Option<Block>,
    /// `[[ArrayBufferMaxByteLength]]` of a resizable buffer.
    pub max: Option<usize>,
}

/// A thread blocked in `Atomics.wait` on the byte at `index`.
struct Waiter {
    index: usize,
    ticket: u64,
    woken: bool,
}

pub(crate) struct SharedState {
    pub block: Block,
    waiters: Vec<Waiter>,
    next_ticket: u64,
}

/// `[[ArrayBufferData]]` of a SharedArrayBuffer, shareable across threads. Atomics hold
/// the mutex for the whole read-modify-write, which makes them atomic.
pub(crate) struct SharedBuffer {
    state: Mutex<SharedState>,
    woken: Condvar,
    /// `[[ArrayBufferMaxByteLength]]` of a growable buffer.
    pub max: Option<usize>,
}

/// Outcome of `Atomics.wait`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WaitResult {
    Ok,
    NotEqual,
    TimedOut,
}

impl SharedBuffer {
    pub(crate) fn new(block: Block, max: Option<usize>) -> Self {
        let state = SharedState {
            block,
            waiters: Vec::new(),
            next_ticket: 0,
        };
        Self {
            state: Mutex::new(state),
            woken: Condvar::new(),
            max,
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, SharedState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Blocks while `holds(bytes)` was true on entry, until notified or `timeout` passes.
    pub(crate) fn wait(
        &self,
        index: usize,
        holds: impl FnOnce(&[u8]) -> bool,
        timeout: Option<Duration>,
    ) -> WaitResult {
        let mut state = self.lock();
        if !holds(&state.block.bytes) {
            return WaitResult::NotEqual;
        }
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.waiters.push(Waiter {
            index,
            ticket,
            woken: false,
        });
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        loop {
            let position = state.waiters.iter().position(|w| w.ticket == ticket);
            let position = position.expect("a waiter stays listed until it leaves");
            let remaining = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            if state.waiters[position].woken || remaining == Some(Duration::ZERO) {
                let waiter = state.waiters.remove(position);
                return match waiter.woken {
                    true => WaitResult::Ok,
                    false => WaitResult::TimedOut,
                };
            }
            state = match remaining {
                Some(remaining) => {
                    let result = self.woken.wait_timeout(state, remaining);
                    result.unwrap_or_else(PoisonError::into_inner).0
                }
                None => self
                    .woken
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
            };
        }
    }

    /// Wakes up to `count` waiters on `index`, oldest first; returns how many woke.
    pub(crate) fn notify(&self, index: usize, count: usize) -> usize {
        let mut state = self.lock();
        let mut woken = 0;
        for waiter in state.waiters.iter_mut() {
            if woken == count {
                break;
            }
            if waiter.index == index && !waiter.woken {
                waiter.woken = true;
                woken += 1;
            }
        }
        if woken > 0 {
            self.woken.notify_all();
        }
        woken
    }
}

#[derive(Clone)]
pub(crate) enum Store {
    Local(Rc<RefCell<LocalBuffer>>),
    Shared(Arc<SharedBuffer>),
}

impl Store {
    pub(crate) fn is_detached(&self) -> bool {
        matches!(self, Store::Local(buffer) if buffer.borrow().block.is_none())
    }

    /// 0 once detached.
    pub(crate) fn byte_length(&self) -> usize {
        self.with_bytes(|bytes| bytes.len())
    }

    pub(crate) fn max(&self) -> Option<usize> {
        match self {
            Store::Local(buffer) => buffer.borrow().max,
            Store::Shared(buffer) => buffer.max,
        }
    }

    /// Runs `f` on the bytes (empty once detached), under the lock for a shared buffer.
    pub(crate) fn with_bytes<R>(&self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        match self {
            Store::Local(buffer) => match buffer.borrow_mut().block.as_mut() {
                Some(block) => f(&mut block.bytes),
                None => f(&mut []),
            },
            Store::Shared(buffer) => f(&mut buffer.lock().block.bytes),
        }
    }
}

/// A window on a buffer: a TypedArray (counted in elements) or a DataView (in bytes).
pub(crate) struct View {
    pub buffer: ObjectRef,
    pub store: Store,
    pub byte_offset: usize,
    /// Fixed length; `None` tracks the length of a resizable buffer.
    pub fixed: Option<usize>,
}

impl View {
    /// Length in `size`-byte units; `None` when detached or out of bounds.
    pub(crate) fn length(&self, size: usize) -> Option<usize> {
        if self.store.is_detached() {
            return None;
        }
        let available = self.store.byte_length().checked_sub(self.byte_offset)?;
        match self.fixed {
            Some(length) => (length * size <= available).then_some(length),
            None => Some(available / size),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn blocks_are_charged_to_the_budget_until_dropped() {
        let budget = Budget::unlimited();
        budget.set_limit(10);
        let mut block = Block::new(&budget, 6).expect("fits");
        assert!(Block::new(&budget, 5).is_none());
        assert!(!block.resize(11));
        assert_eq!(block.bytes.len(), 6);
        assert!(block.resize(2));
        let other = Block::new(&budget, 8).expect("fits after the shrink");
        drop(block);
        drop(other);
        assert!(Block::new(&budget, 10).is_some());
    }

    #[test]
    fn wait_returns_after_notify_from_another_thread() {
        let budget = Budget::unlimited();
        let shared = Arc::new(SharedBuffer::new(Block::new(&budget, 4).unwrap(), None));
        let waiter = {
            let shared = shared.clone();
            thread::spawn(move || shared.wait(0, |bytes| bytes[0] == 0, None))
        };
        while shared.lock().waiters.is_empty() {
            thread::yield_now();
        }
        assert_eq!(shared.notify(0, usize::MAX), 1);
        assert_eq!(waiter.join().unwrap(), WaitResult::Ok);
        assert_eq!(
            shared.wait(0, |bytes| bytes[0] == 1, None),
            WaitResult::NotEqual
        );
        let timeout = Some(Duration::from_millis(1));
        assert_eq!(shared.wait(0, |_| true, timeout), WaitResult::TimedOut);
        assert_eq!(shared.notify(0, 1), 0);
    }
}

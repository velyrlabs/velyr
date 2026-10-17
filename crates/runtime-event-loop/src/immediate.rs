//! `setImmediate` queue: callbacks run in the loop's check phase (after I/O, before timers).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use velyr_engine_api::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImmediateId(pub u64);

#[derive(Default)]
pub(crate) struct Immediates {
    next_id: u64,
    /// id -> ref'd; ordered by id, which is creation order.
    queue: BTreeMap<u64, bool>,
    /// Queue entries that are ref'd, so `has_ref` does not scan the queue.
    refd_count: usize,
    dispatcher: Option<Value>,
}

impl Immediates {
    /// Removes `id` if queued; true when it was.
    fn remove(&mut self, id: u64) -> bool {
        let Some(refd) = self.queue.remove(&id) else {
            return false;
        };
        self.refd_count -= usize::from(refd);
        true
    }
}

/// Isolate-thread handle for host functions to schedule immediates. When one runs, the loop
/// calls the dispatcher set with [`set_dispatcher`](Self::set_dispatcher) as `dispatcher(id)`.
#[derive(Clone, Default)]
pub struct ImmediateHandle(Rc<RefCell<Immediates>>);

impl ImmediateHandle {
    pub fn set(&self) -> ImmediateId {
        let mut shared = self.0.borrow_mut();
        shared.next_id += 1;
        let id = shared.next_id;
        shared.queue.insert(id, true);
        shared.refd_count += 1;
        ImmediateId(id)
    }

    pub fn clear(&self, id: ImmediateId) {
        self.0.borrow_mut().remove(id.0);
    }

    pub fn set_ref(&self, id: ImmediateId, refd: bool) {
        let mut shared = self.0.borrow_mut();
        let Some(slot) = shared.queue.get_mut(&id.0) else {
            return;
        };
        if *slot != refd {
            *slot = refd;
            if refd {
                shared.refd_count += 1;
            } else {
                shared.refd_count -= 1;
            }
        }
    }

    pub fn set_dispatcher(&self, dispatcher: Value) {
        self.0.borrow_mut().dispatcher = Some(dispatcher);
    }

    pub(crate) fn has_ref(&self) -> bool {
        self.0.borrow().refd_count > 0
    }

    pub(crate) fn dispatcher(&self) -> Option<Value> {
        self.0.borrow().dispatcher.clone()
    }

    /// Ids queued at the start of a check phase; later ones wait for the next tick.
    pub(crate) fn snapshot(&self) -> Vec<u64> {
        self.0.borrow().queue.keys().copied().collect()
    }

    /// Removes `id` if it is still queued (it may have been cleared by an earlier callback).
    pub(crate) fn take(&self, id: u64) -> bool {
        self.0.borrow_mut().remove(id)
    }
}

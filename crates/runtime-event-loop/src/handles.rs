//! Ref state of keyed ops: an op started on a key whose handle is unref'd does not keep the
//! loop alive, but is still delivered while the loop runs for another reason.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A handle's state outlives its ops: a key unref'd once stays unref'd for later ops.
struct Handle {
    refd: bool,
    ops: usize,
}

#[derive(Default)]
pub(crate) struct Handles {
    map: Mutex<HashMap<u64, Handle>>,
    /// Ops of unref'd handles; written under the `map` lock, read without it.
    unrefd: AtomicUsize,
}

impl Handles {
    pub(crate) fn begin(&self, key: u64) {
        let mut map = self.map();
        let handle = map.entry(key).or_insert(Handle { refd: true, ops: 0 });
        handle.ops += 1;
        if !handle.refd {
            self.unrefd.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(crate) fn end(&self, key: u64) {
        let mut map = self.map();
        let Some(handle) = map.get_mut(&key) else {
            return;
        };
        if handle.ops > 0 {
            handle.ops -= 1;
            if !handle.refd {
                self.unrefd.fetch_sub(1, Ordering::SeqCst);
            }
        }
        if handle.ops == 0 && handle.refd {
            map.remove(&key);
        }
    }

    pub(crate) fn set_ref(&self, key: u64, refd: bool) {
        let mut map = self.map();
        match map.get_mut(&key) {
            Some(handle) => {
                if handle.refd != refd {
                    self.shift_unrefd(handle.ops, refd);
                }
                handle.refd = refd;
            }
            None if !refd => {
                map.insert(key, Handle { refd, ops: 0 });
            }
            None => {}
        }
        // A ref'd handle with no ops carries no state: only `key` can have become one.
        if refd && map.get(&key).is_some_and(|handle| handle.ops == 0) {
            map.remove(&key);
        }
    }

    /// Pending ops that do not keep the loop alive.
    pub(crate) fn unrefd_ops(&self) -> usize {
        self.unrefd.load(Ordering::SeqCst)
    }

    /// Moves `ops` out of (`refd`) or into the unref'd count.
    fn shift_unrefd(&self, ops: usize, refd: bool) {
        if refd {
            self.unrefd.fetch_sub(ops, Ordering::SeqCst);
        } else {
            self.unrefd.fetch_add(ops, Ordering::SeqCst);
        }
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Handle>> {
        self.map
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

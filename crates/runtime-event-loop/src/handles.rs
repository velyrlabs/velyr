//! Ref state of keyed ops: an op started on a key whose handle is unref'd does not keep the
//! loop alive, but is still delivered while the loop runs for another reason.

use std::collections::HashMap;
use std::sync::Mutex;

/// A handle's state outlives its ops: a key unref'd once stays unref'd for later ops.
struct Handle {
    refd: bool,
    ops: usize,
}

#[derive(Default)]
pub(crate) struct Handles(Mutex<HashMap<u64, Handle>>);

impl Handles {
    pub(crate) fn begin(&self, key: u64) {
        self.map()
            .entry(key)
            .or_insert(Handle { refd: true, ops: 0 })
            .ops += 1;
    }

    pub(crate) fn end(&self, key: u64) {
        let mut map = self.map();
        let Some(handle) = map.get_mut(&key) else {
            return;
        };
        handle.ops = handle.ops.saturating_sub(1);
        if handle.ops == 0 && handle.refd {
            map.remove(&key);
        }
    }

    pub(crate) fn set_ref(&self, key: u64, refd: bool) {
        let mut map = self.map();
        match map.get_mut(&key) {
            Some(handle) => handle.refd = refd,
            None if !refd => {
                map.insert(key, Handle { refd, ops: 0 });
            }
            None => {}
        }
        if refd {
            map.retain(|_, handle| !handle.refd || handle.ops > 0);
        }
    }

    /// Pending ops that do not keep the loop alive.
    pub(crate) fn unrefd_ops(&self) -> usize {
        self.map().values().filter(|h| !h.refd).map(|h| h.ops).sum()
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Handle>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

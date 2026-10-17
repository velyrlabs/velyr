//! One heap budget shared by every worker of a main isolate, nested workers included (RT-18c).
//!
//! A worker reserves its heap limit from the pool before it starts and gives it back when its
//! isolate is dropped, so all live workers together never exceed the pool. Hosts size the pool at
//! the main isolate's own limit: the main isolate plus its workers then stay under twice that
//! limit, however many workers the `workers` quota allows (before, each got the full limit).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use velyr_runtime_errors::VelyrError;

use crate::auto::PER_WORKER_ESTIMATE_BYTES;

/// Heap limit of a worker that asks for none: the estimate `auto` sizes worker counts with.
pub const DEFAULT_WORKER_HEAP_BYTES: usize = PER_WORKER_ESTIMATE_BYTES as usize;
/// Smallest limit worth starting an isolate with; a spawn that cannot get it fails.
pub const MIN_WORKER_HEAP_BYTES: usize = 32 << 20;

pub struct HeapPool {
    remaining: AtomicUsize,
}

impl HeapPool {
    pub fn new(total_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            remaining: AtomicUsize::new(total_bytes),
        })
    }

    pub fn remaining(&self) -> usize {
        self.remaining.load(Ordering::Acquire)
    }

    /// Reserves `requested` bytes (default [`DEFAULT_WORKER_HEAP_BYTES`]), capped at what is
    /// left. Fails with a `heap` quota error when less than `min(requested, MIN)` is left.
    pub fn reserve(self: &Arc<Self>, requested: Option<usize>) -> Result<HeapGrant, VelyrError> {
        let wanted = requested.unwrap_or(DEFAULT_WORKER_HEAP_BYTES);
        let floor = wanted.min(MIN_WORKER_HEAP_BYTES);
        let mut bytes = 0;
        self.remaining
            .try_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                bytes = wanted.min(left);
                (bytes >= floor).then(|| left - bytes)
            })
            .map_err(|_| VelyrError::quota_exceeded("heap"))?;
        Ok(HeapGrant {
            pool: Arc::clone(self),
            bytes,
        })
    }
}

/// Heap reserved for one worker; returned to the pool on drop.
pub struct HeapGrant {
    pool: Arc<HeapPool>,
    bytes: usize,
}

impl HeapGrant {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for HeapGrant {
    fn drop(&mut self) {
        self.pool.remaining.fetch_add(self.bytes, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1 << 20;

    #[test]
    fn workers_share_the_pool_instead_of_each_getting_all_of_it() {
        let pool = HeapPool::new(256 * MIB);
        let first = pool.reserve(None).unwrap();
        let second = pool.reserve(None).unwrap();
        assert_eq!(first.bytes() + second.bytes(), 256 * MIB);
        assert!(pool.reserve(None).is_err(), "the pool is spent");
        drop(first);
        assert_eq!(
            pool.reserve(None).unwrap().bytes(),
            DEFAULT_WORKER_HEAP_BYTES
        );
    }

    #[test]
    fn a_request_is_capped_at_what_is_left_down_to_the_floor() {
        let pool = HeapPool::new(100 * MIB);
        let big = pool.reserve(Some(60 * MIB)).unwrap();
        assert_eq!(pool.reserve(Some(60 * MIB)).unwrap().bytes(), 40 * MIB);
        drop(big);
        let _rest = pool.reserve(Some(90 * MIB)).unwrap();
        let err = pool.reserve(None).err().expect("10 MiB is under the floor");
        assert_eq!(
            err.to_string(),
            VelyrError::quota_exceeded("heap").to_string()
        );
        assert_eq!(pool.reserve(Some(8 * MIB)).unwrap().bytes(), 8 * MIB);
    }

    #[test]
    fn grants_never_exceed_the_pool_however_many_workers_spawn() {
        let pool = HeapPool::new(4 << 30);
        let grants: Vec<_> = (0..64).map_while(|_| pool.reserve(None).ok()).collect();
        let total: usize = grants.iter().map(HeapGrant::bytes).sum();
        assert!(total <= 4 << 30, "{total}");
        drop(grants);
        assert_eq!(pool.remaining(), 4 << 30);
    }
}

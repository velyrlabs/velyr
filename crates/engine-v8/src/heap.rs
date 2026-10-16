//! Near-heap-limit handling: without a callback V8 aborts the whole process on OOM.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Extra room granted once the limit is hit, so V8 can unwind the terminated execution.
const UNWIND_HEADROOM_BYTES: usize = 8 * 1024 * 1024;

/// Passed to V8 as callback data; must outlive the isolate.
pub(crate) struct HeapGuard {
    exhausted: Arc<AtomicBool>,
    handle: v8::IsolateHandle,
}

impl HeapGuard {
    /// Registers the callback. `exhausted` is set when the limit is hit.
    pub(crate) fn install(isolate: &mut v8::Isolate, exhausted: Arc<AtomicBool>) -> Box<Self> {
        let guard = Box::new(Self {
            exhausted,
            handle: isolate.thread_safe_handle(),
        });
        let data = std::ptr::from_ref::<Self>(&guard)
            .cast_mut()
            .cast::<c_void>();
        isolate.add_near_heap_limit_callback(on_near_heap_limit, data);
        guard
    }
}

#[allow(unsafe_code)]
unsafe extern "C" fn on_near_heap_limit(
    data: *mut c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    // SAFETY: `data` is the `Box<HeapGuard>` owned by `V8Isolate`, which is dropped after
    // the isolate, so it is alive for every GC that can invoke this callback.
    let guard = unsafe { &*data.cast::<HeapGuard>() };
    guard.exhausted.store(true, Ordering::Release);
    guard.handle.terminate_execution();
    current_heap_limit + UNWIND_HEADROOM_BYTES
}

//! The per-isolate Tokio runtime and its bounded blocking pool (RT-19).

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::runtime::{Builder, Runtime};

const MAX_DEFAULT_THREADS: usize = 8;
const KEEP_ALIVE: Duration = Duration::from_secs(10);

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Most blocking threads alive at once in this process since start. Test hook.
pub fn peak_blocking_threads() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// `min(8, available_parallelism)`, the default per-isolate cap.
fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(MAX_DEFAULT_THREADS, |n| n.get().min(MAX_DEFAULT_THREADS))
}

pub(crate) fn build_runtime(max_threads: Option<usize>) -> io::Result<Runtime> {
    Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .max_blocking_threads(max_threads.unwrap_or_else(default_threads).max(1))
        .thread_name("velyr-blocking")
        .thread_keep_alive(KEEP_ALIVE)
        .on_thread_start(|| {
            let live = LIVE.fetch_add(1, Ordering::Relaxed) + 1;
            PEAK.fetch_max(live, Ordering::Relaxed);
        })
        .on_thread_stop(|| {
            LIVE.fetch_sub(1, Ordering::Relaxed);
        })
        .build()
}

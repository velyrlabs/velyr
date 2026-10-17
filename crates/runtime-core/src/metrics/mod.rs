//! Runtime metrics (TDD section 17), produced from Rust and readable from any thread: the
//! numbers live in atomics (and a briefly locked heap copy) that the isolate thread updates, so a
//! scrape (the CLI exporter) still answers while JS is CPU-saturated. `Velyr.metrics()` returns
//! the same [`Snapshot`].
//!
//! GC count, pause, cells and external bytes are reported where the engine has them
//! (engine-native; V8 gives external bytes only). Not covered: per-worker utilization. Heap is sampled when a JS turn ends, so it is stale while one runs. Server
//! workers (`serve --workers`) register an id: their loop lag and heap are reported per
//! `worker` label, while requests, pending ops and resources are totals over the process.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use velyr_engine_api::HeapStats;
use velyr_runtime_event_loop::CompletionSender;
use velyr_runtime_resources::{QuotaClass, QuotaContext};

mod layer;
mod ops;
mod render;

pub use layer::MetricsLayer;
pub use ops::{install_metrics, metrics_host_functions};

/// Upper bounds, in seconds, of the HTTP latency histogram buckets (`+Inf` is implicit).
pub const HTTP_BUCKETS_SECONDS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Quota classes reported as active resources.
const RESOURCE_CLASSES: [QuotaClass; 8] = [
    QuotaClass::Files,
    QuotaClass::Sockets,
    QuotaClass::Listeners,
    QuotaClass::Workers,
    QuotaClass::Timers,
    QuotaClass::Requests,
    QuotaClass::Compression,
    QuotaClass::Processes,
];

const STATUS_CLASSES: [&str; 5] = ["1xx", "2xx", "3xx", "4xx", "5xx"];

struct Sources {
    sender: CompletionSender,
    quotas: Arc<QuotaContext>,
}

/// What one isolate's thread writes: loop lag, heap and the sources behind its resource counts.
#[derive(Default)]
struct IsolateMetrics {
    /// Nanoseconds since `start` at which the running JS turn began, plus one; 0 when idle.
    busy_since: AtomicU64,
    last_turn_nanos: AtomicU64,
    /// `None` until the first turn ended; locked only to copy the stats in or out.
    heap: Mutex<Option<HeapStats>>,
    sources: Mutex<Option<Sources>>,
}

impl IsolateMetrics {
    fn lag_nanos(&self, now: u64) -> u64 {
        let began = self.busy_since.load(Ordering::Acquire);
        // Busy: how long the current turn has held the loop; idle: the last turn's length.
        if began > 0 {
            now.saturating_sub(began - 1)
        } else {
            self.last_turn_nanos.load(Ordering::Acquire)
        }
    }

    fn heap(&self) -> Option<HeapStats> {
        *self.heap.lock().expect("metrics heap")
    }
}

thread_local! {
    /// The server worker slot of this thread, set by [`Metrics::register_worker`].
    static WORKER_SLOT: RefCell<Option<Arc<IsolateMetrics>>> = const { RefCell::new(None) };
}

/// Process-wide metric state; see [`global`].
pub struct Metrics {
    start: Instant,
    /// Isolates without a worker id (`velyr run`, JS workers).
    main: IsolateMetrics,
    /// Server workers by id; a restarted worker replaces its slot.
    workers: Mutex<BTreeMap<usize, Arc<IsolateMetrics>>>,
    permission_denied: AtomicU64,
    quota_denied: AtomicU64,
    http_by_status: [AtomicU64; 5],
    /// Per-bucket counts (not cumulative); the last slot is `+Inf`.
    http_buckets: [AtomicU64; HTTP_BUCKETS_SECONDS.len() + 1],
    http_sum_micros: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            main: IsolateMetrics::default(),
            workers: Mutex::new(BTreeMap::new()),
            permission_denied: AtomicU64::new(0),
            quota_denied: AtomicU64::new(0),
            http_by_status: Default::default(),
            http_buckets: Default::default(),
            http_sum_micros: AtomicU64::new(0),
        }
    }

    fn nanos(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    /// Runs `write` on this thread's worker slot, or on the main one without a worker id.
    fn current<R>(&self, write: impl FnOnce(&IsolateMetrics) -> R) -> R {
        WORKER_SLOT.with(|slot| match &*slot.borrow() {
            Some(worker) => write(worker),
            None => write(&self.main),
        })
    }

    /// Makes the calling thread server worker `id`: its turns, heap and `bind` land in their own
    /// slot, reported with a `worker` label. Call it on the isolate's thread before the isolate
    /// starts; a restarted worker calls it again and replaces the old slot.
    pub fn register_worker(&self, id: usize) {
        let slot = Arc::new(IsolateMetrics::default());
        self.workers
            .lock()
            .expect("metrics workers")
            .insert(id, slot.clone());
        WORKER_SLOT.with(|current| *current.borrow_mut() = Some(slot));
    }

    /// The calling thread's worker identity, for the isolate thread it spawns: pass the result to
    /// [`WorkerSlot::enter`] on that thread. Without a worker id it does nothing.
    pub fn worker_slot() -> WorkerSlot {
        WorkerSlot(WORKER_SLOT.with(|slot| slot.borrow().clone()))
    }

    /// A JS turn (script, module, call or microtask checkpoint) starts running.
    pub fn turn_start(&self) {
        let now = self.nanos() + 1;
        self.current(|isolate| isolate.busy_since.store(now, Ordering::Release));
    }

    /// The turn that [`turn_start`](Self::turn_start) announced ended.
    pub fn turn_end(&self) {
        let now = self.nanos();
        self.current(|isolate| {
            let began = isolate.busy_since.swap(0, Ordering::AcqRel);
            if began > 0 {
                isolate
                    .last_turn_nanos
                    .store(now.saturating_sub(began - 1), Ordering::Release);
            }
        });
    }

    pub fn record_heap(&self, stats: HeapStats) {
        self.current(|isolate| *isolate.heap.lock().expect("metrics heap") = Some(stats));
    }

    pub fn count_permission_denied(&self) {
        self.permission_denied.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_quota_denied(&self) {
        self.quota_denied.fetch_add(1, Ordering::Relaxed);
    }

    /// One answered HTTP request: its status and the time from receiving it to the response head.
    pub fn record_http_request(&self, status: u16, elapsed: Duration) {
        let class = usize::from(status / 100).clamp(1, 5) - 1;
        self.http_by_status[class].fetch_add(1, Ordering::Relaxed);
        let seconds = elapsed.as_secs_f64();
        let bucket = HTTP_BUCKETS_SECONDS
            .iter()
            .position(|bound| seconds <= *bound)
            .unwrap_or(HTTP_BUCKETS_SECONDS.len());
        self.http_buckets[bucket].fetch_add(1, Ordering::Relaxed);
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.http_sum_micros.fetch_add(micros, Ordering::Relaxed);
    }

    /// Gives snapshots the isolate's pending ops and resource counts; the host calls it once the
    /// isolate exists (the `install` hook).
    pub fn bind(&self, sender: CompletionSender, quotas: Arc<QuotaContext>) {
        self.current(|isolate| {
            *isolate.sources.lock().expect("metrics sources") = Some(Sources { sender, quotas });
        });
    }

    pub fn snapshot(&self) -> Snapshot {
        let now = self.nanos();
        let workers: Vec<(usize, Arc<IsolateMetrics>)> = self
            .workers
            .lock()
            .expect("metrics workers")
            .iter()
            .map(|(id, slot)| (*id, slot.clone()))
            .collect();
        // Pending ops and resources are summed over every isolate that bound its sources.
        let mut pending_ops = 0;
        let mut used = [0; RESOURCE_CLASSES.len()];
        let mut bound = false;
        for isolate in std::iter::once(&self.main).chain(workers.iter().map(|(_, slot)| &**slot)) {
            if let Some(sources) = &*isolate.sources.lock().expect("metrics sources") {
                bound = true;
                pending_ops += sources.sender.pending_ops() as u64;
                for (total, class) in used.iter_mut().zip(RESOURCE_CLASSES) {
                    *total += sources.quotas.used(class);
                }
            }
        }
        let resources = if bound {
            RESOURCE_CLASSES
                .iter()
                .zip(used)
                .map(|(class, used)| (class.name(), used))
                .collect()
        } else {
            Vec::new()
        };
        let mut cumulative = 0;
        let buckets = self
            .http_buckets
            .iter()
            .map(|count| {
                cumulative += count.load(Ordering::Relaxed);
                cumulative
            })
            .collect();
        Snapshot {
            rss_bytes: memory_stats::memory_stats().map(|stats| stats.physical_mem as u64),
            heap: self.main.heap(),
            loop_lag_seconds: self.main.lag_nanos(now) as f64 / 1e9,
            pending_ops,
            resources,
            workers: workers
                .iter()
                .map(|(id, slot)| WorkerSnapshot {
                    id: *id,
                    heap: slot.heap(),
                    loop_lag_seconds: slot.lag_nanos(now) as f64 / 1e9,
                })
                .collect(),
            permission_denied: self.permission_denied.load(Ordering::Relaxed),
            quota_denied: self.quota_denied.load(Ordering::Relaxed),
            http: HttpSnapshot {
                by_status: std::array::from_fn(|i| self.http_by_status[i].load(Ordering::Relaxed)),
                buckets,
                sum_seconds: self.http_sum_micros.load(Ordering::Relaxed) as f64 / 1e6,
            },
        }
    }
}

/// The process-wide instance every isolate and the exporter share.
pub fn global() -> &'static Arc<Metrics> {
    static GLOBAL: OnceLock<Arc<Metrics>> = OnceLock::new();
    GLOBAL.get_or_init(|| Arc::new(Metrics::new()))
}

/// HTTP request counters and latency histogram.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HttpSnapshot {
    /// Requests per status class, in [`STATUS_CLASSES`] order (1xx to 5xx).
    pub by_status: [u64; 5],
    /// Cumulative counts per [`HTTP_BUCKETS_SECONDS`] bound; the last entry is `+Inf` (the total).
    pub buckets: Vec<u64>,
    pub sum_seconds: f64,
}

impl HttpSnapshot {
    pub fn requests(&self) -> u64 {
        self.by_status.iter().sum()
    }
}

/// One reading of every metric.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    /// Resident set size; `None` where the platform gives no reading.
    pub rss_bytes: Option<u64>,
    /// `None` until the first JS turn ended.
    pub heap: Option<HeapStats>,
    pub loop_lag_seconds: f64,
    pub pending_ops: u64,
    /// Open resources per quota class (files, sockets, listeners, workers, timers).
    pub resources: Vec<(&'static str, u64)>,
    /// Loop lag and heap of each registered server worker; empty outside `serve --workers`.
    pub workers: Vec<WorkerSnapshot>,
    pub permission_denied: u64,
    pub quota_denied: u64,
    pub http: HttpSnapshot,
}

/// Loop lag and heap of one server worker.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkerSnapshot {
    pub id: usize,
    pub heap: Option<HeapStats>,
    pub loop_lag_seconds: f64,
}

/// A worker identity moved to the isolate thread; see [`Metrics::worker_slot`].
pub struct WorkerSlot(Option<Arc<IsolateMetrics>>);

impl WorkerSlot {
    pub fn enter(self) {
        if self.0.is_some() {
            WORKER_SLOT.with(|current| *current.borrow_mut() = self.0);
        }
    }
}

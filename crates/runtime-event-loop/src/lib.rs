//! Per-isolate event loop: completion queue, fairness and tick ordering (ADR-010).
//!
//! The loop is a future driven on the isolate thread's Tokio `current_thread`
//! runtime. Native tasks on any thread report back through a [`CompletionSender`];
//! results become engine values only on the isolate thread.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use velyr_engine_api::{GcReason, Isolate, JsString, ResolverId, TerminationHandle, Value};
use velyr_runtime_errors::VelyrError;

mod handles;
mod immediate;
mod timers;

use handles::Handles;
pub use immediate::{ImmediateHandle, ImmediateId};
use timers::{TimerCommand, Timers};
pub use timers::{TimerHandle, TimerId};

pub const DEFAULT_MAX_COMPLETIONS_PER_TICK: usize = 1024;
pub const DEFAULT_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);
const LAG_SAMPLES: usize = 1024;
/// Continuous idleness before the loop asks for a GC. On the large-app bench this cut the idle
/// footprint by 11% (1 worker) and 15% (4 workers) for under 2% CPU per request (VELYR-215).
const IDLE_GC_DELAY: Duration = Duration::from_secs(1);
const IDLE_GC_MIN_GROWTH: usize = 4 << 20;
/// Polling period while the engine has background work outstanding (async wasm compiles).
const PLATFORM_POLL: Duration = Duration::from_millis(1);

/// `Send` payload of a finished host operation.
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Bytes(Vec<u8>),
}

/// Outcome of one async op, addressed to the promise it settles.
#[derive(Debug)]
pub struct Completion {
    pub resolver: ResolverId,
    pub result: Result<CompletionValue, VelyrError>,
}

/// Cloneable, thread-safe producer side of the completion queue.
#[derive(Clone)]
pub struct CompletionSender {
    tx: mpsc::UnboundedSender<Completion>,
    pending: Arc<AtomicUsize>,
    handles: Arc<Handles>,
    root: CancellationToken,
}

impl CompletionSender {
    /// Registers an op that keeps the loop alive until its guard sends or is dropped.
    /// After termination the guard's token is already cancelled.
    pub fn begin_op(&self) -> OpGuard {
        self.pending.fetch_add(1, Ordering::SeqCst);
        self.guard(None)
    }

    /// Like [`begin_op`](Self::begin_op), for an op of the handle `key`: while that handle is
    /// unref'd (see [`set_handle_ref`](Self::set_handle_ref)) the op does not keep the loop alive.
    pub fn begin_op_on(&self, key: u64) -> OpGuard {
        // Mirror of `OpGuard::drop`, which releases the key first: a racing read only over-counts.
        self.pending.fetch_add(1, Ordering::SeqCst);
        self.handles.begin(key);
        self.guard(Some(key))
    }

    /// Refs or unrefs every op of `key`, started or future. Handles start ref'd.
    pub fn set_handle_ref(&self, key: u64, refd: bool) {
        self.handles.set_ref(key, refd);
    }

    fn guard(&self, key: Option<u64>) -> OpGuard {
        OpGuard {
            tx: self.tx.clone(),
            pending: self.pending.clone(),
            handles: self.handles.clone(),
            key,
            token: self.root.child_token(),
            sent: false,
        }
    }

    /// Ops started and not completed yet; safe to read from any thread (metrics).
    pub fn pending_ops(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// True once the isolate's loop was terminated or shut down.
    pub fn is_shutdown(&self) -> bool {
        self.root.is_cancelled()
    }
}

/// One in-flight op. Dropping it without [`send`](Self::send) releases the loop.
pub struct OpGuard {
    tx: mpsc::UnboundedSender<Completion>,
    pending: Arc<AtomicUsize>,
    handles: Arc<Handles>,
    key: Option<u64>,
    token: CancellationToken,
    sent: bool,
}

impl OpGuard {
    /// Child of the isolate's token: cancelled on terminate/shutdown.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Delivers the completion. Dropped silently if the loop is gone (isolate terminated).
    pub fn send(mut self, completion: Completion) {
        self.sent = true;
        let _ = self.tx.send(completion);
    }
}

impl Drop for OpGuard {
    fn drop(&mut self) {
        if let Some(key) = self.key {
            self.handles.end(key);
        }
        if !self.sent {
            // Saturating: `terminate` resets the counter while guards are still alive.
            let _ = self
                .pending
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopStats {
    pub lag_p50_ms: f64,
    pub lag_max_ms: f64,
    pub pending_ops: usize,
    pub ticks: u64,
}

pub struct EventLoop<I: Isolate> {
    isolate: I,
    rx: mpsc::UnboundedReceiver<Completion>,
    tx: mpsc::UnboundedSender<Completion>,
    pending: Arc<AtomicUsize>,
    handles: Arc<Handles>,
    shutdown: CancellationToken,
    termination: TerminationHandle,
    shutdown_deadline: Duration,
    max_completions_per_tick: usize,
    batch: Vec<Completion>,
    lags_ms: VecDeque<f64>,
    lag_max_ms: f64,
    ticks: u64,
    timers: Timers,
    host_timers: TimerHandle,
    immediates: ImmediateHandle,
    heap_after_idle_gc: usize,
    platform_busy: bool,
}

impl<I: Isolate> EventLoop<I> {
    pub fn new(isolate: I, shutdown: CancellationToken) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let host_timers = TimerHandle::default();
        Self {
            termination: isolate.termination_handle(),
            isolate,
            rx,
            tx,
            pending: Arc::new(AtomicUsize::new(0)),
            handles: Arc::default(),
            shutdown,
            shutdown_deadline: DEFAULT_SHUTDOWN_DEADLINE,
            max_completions_per_tick: DEFAULT_MAX_COMPLETIONS_PER_TICK,
            batch: Vec::new(),
            lags_ms: VecDeque::new(),
            lag_max_ms: 0.0,
            ticks: 0,
            timers: Timers::default(),
            immediates: host_timers.immediates(),
            host_timers,
            heap_after_idle_gc: 0,
            platform_busy: false,
        }
    }

    pub fn with_max_completions_per_tick(mut self, max: usize) -> Self {
        self.max_completions_per_tick = max.max(1);
        self
    }

    pub fn with_shutdown_deadline(mut self, deadline: Duration) -> Self {
        self.shutdown_deadline = deadline;
        self
    }

    pub fn sender(&self) -> CompletionSender {
        CompletionSender {
            tx: self.tx.clone(),
            pending: self.pending.clone(),
            handles: self.handles.clone(),
            root: self.shutdown.clone(),
        }
    }

    /// Schedules `callback` (called with `this = undefined`) after `delay`; repeats if `repeat`.
    pub fn set_timer(&mut self, delay: Duration, repeat: bool, callback: Value) -> TimerId {
        let id = self.host_timers.0.borrow_mut().alloc_id();
        self.timers.add(id, Instant::now(), delay, repeat, callback)
    }

    /// Handle for host functions to schedule timers from JS (see [`TimerHandle`]).
    pub fn timer_handle(&self) -> TimerHandle {
        self.host_timers.clone()
    }

    pub fn clear_timer(&mut self, id: TimerId) {
        if let Some(callback) = self.timers.clear(id) {
            self.release(&callback);
        }
    }

    /// An unref'd timer still fires while the loop is alive but does not keep it alive.
    pub fn set_ref(&mut self, id: TimerId, refd: bool) {
        self.timers.set_ref(id, refd);
    }

    pub fn isolate(&mut self) -> &mut I {
        &mut self.isolate
    }

    pub fn stats(&self) -> LoopStats {
        let mut sorted: Vec<f64> = self.lags_ms.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        LoopStats {
            lag_p50_ms: sorted.get(sorted.len() / 2).copied().unwrap_or(0.0),
            lag_max_ms: self.lag_max_ms,
            pending_ops: self.pending.load(Ordering::SeqCst),
            ticks: self.ticks,
        }
    }

    /// Pending ops that keep the loop alive: all but those of unref'd handles.
    fn live_ops(&self) -> usize {
        // Unref'd count first: a racing release then only ever over-counts.
        let unrefd = self.handles.unrefd_ops();
        self.pending.load(Ordering::SeqCst).saturating_sub(unrefd)
    }

    /// Hard stop: cancels every op token, interrupts running JS, closes the queue and
    /// discards queued and late completions. Completions sent afterwards fail silently.
    pub fn terminate(&mut self) {
        self.shutdown.cancel();
        self.termination.terminate();
        self.rx.close();
        while self.rx.try_recv().is_ok() {}
        self.batch.clear();
        self.pending.store(0, Ordering::SeqCst);
    }

    /// Cancels op tokens and applies completions until no op is pending or the shutdown
    /// deadline passes, then forces [`terminate`](Self::terminate).
    pub async fn shutdown(&mut self) {
        self.shutdown.cancel();
        let deadline = self.shutdown_deadline;
        let _ = tokio::time::timeout(deadline, self.drain()).await;
        self.terminate();
    }

    async fn drain(&mut self) {
        let max = self.max_completions_per_tick;
        while self.pending.load(Ordering::SeqCst) > 0 {
            if self.rx.recv_many(&mut self.batch, max).await == 0 {
                return;
            }
            self.tick();
        }
    }

    /// One iteration; returns how many completions were applied (at most the per-tick cap).
    pub fn tick(&mut self) -> usize {
        let _span = tracing::debug_span!("velyr.event_loop.tick").entered();
        self.platform_busy = self.isolate.run_platform_tasks();
        self.isolate.perform_microtask_checkpoint();
        self.apply_host_timer_commands();
        while self.batch.len() < self.max_completions_per_tick {
            match self.rx.try_recv() {
                Ok(completion) => self.batch.push(completion),
                Err(_) => break,
            }
        }
        let batch = std::mem::take(&mut self.batch);
        let processed = batch.len();
        for completion in batch {
            self.pending.fetch_sub(1, Ordering::SeqCst);
            self.apply(completion);
        }
        self.apply_host_timer_commands();
        self.run_immediates();
        self.run_due_timers();
        self.isolate.perform_microtask_checkpoint();
        self.apply_host_timer_commands();
        self.ticks += 1;
        processed
    }

    /// Runs until no op is pending (and no timer is ref'd) or shutdown is requested.
    pub async fn run(&mut self) {
        let mut due = Instant::now();
        loop {
            self.record_lag(due);
            self.tick();
            if self.shutdown.is_cancelled() {
                return;
            }
            if !self.rx.is_empty() {
                due = Instant::now();
                tokio::task::yield_now().await;
                continue;
            }
            if self.immediates.has_ref() {
                due = Instant::now();
                tokio::task::yield_now().await;
                continue;
            }
            if self.live_ops() == 0
                && !self.timers.has_ref()
                && !self.platform_busy
            {
                return;
            }
            let max = self.max_completions_per_tick;
            let mut wake = self.timers.next_deadline();
            if self.platform_busy {
                // Engine background threads cannot wake the loop: poll while their work lasts.
                let poll = Instant::now() + PLATFORM_POLL;
                wake = Some(wake.map_or(poll, |at| at.min(poll)));
            }
            let mut idle_gc_due = true;
            let timer_fired = loop {
                tokio::select! {
                    _ = self.shutdown.cancelled() => return,
                    _ = self.rx.recv_many(&mut self.batch, max) => break false,
                    _ = sleep_until_deadline(wake) => break true,
                    () = tokio::time::sleep(IDLE_GC_DELAY), if idle_gc_due => {
                        idle_gc_due = false;
                        self.collect_idle_garbage();
                    }
                }
            };
            due = wake.filter(|_| timer_fired).unwrap_or_else(Instant::now);
        }
    }

    /// Gives the heap a burst left behind back to the OS (VELYR-215). Skipped until the heap
    /// grew since the last one, so sparse traffic does not pay a full GC per request. Uses the
    /// used size: right after a full GC it is exact, while the committed size shrinks later.
    fn collect_idle_garbage(&mut self) {
        let used = self.isolate.heap_stats().used_bytes;
        if used < self.heap_after_idle_gc + IDLE_GC_MIN_GROWTH {
            return;
        }
        self.isolate.request_gc(GcReason::Idle);
        self.heap_after_idle_gc = self.isolate.heap_stats().used_bytes;
    }

    /// Check phase: runs the immediates queued when the phase starts, in creation order.
    fn run_immediates(&mut self) {
        let Some(dispatcher) = self.immediates.dispatcher() else {
            return;
        };
        for id in self.immediates.snapshot() {
            if !self.immediates.take(id) {
                continue;
            }
            let args = [Value::Number(id as f64)];
            if let Err(err) = self.isolate.call(&dispatcher, &Value::Undefined, &args) {
                tracing::warn!(message = %err.message, "immediate callback threw");
            }
            self.isolate.perform_microtask_checkpoint();
            self.apply_host_timer_commands();
        }
    }

    fn run_due_timers(&mut self) {
        for id in self.timers.due(Instant::now()) {
            let Some(entry) = self.timers.begin_run(id) else {
                continue;
            };
            let (func, args) = self.call_target(&entry);
            if let Err(err) = self.isolate.call(&func, &Value::Undefined, &args) {
                tracing::warn!(message = %err.message, "timer callback threw");
            }
            // Before `end_run`: timers set by the callback inherit its nesting level, and a
            // `clear` of the running interval must stop the reschedule.
            self.apply_host_timer_commands();
            if let Some(callback) = self.timers.end_run(Instant::now(), entry) {
                self.release(&callback);
            }
            self.isolate.perform_microtask_checkpoint();
            self.apply_host_timer_commands();
        }
    }

    /// Host timers have no callback of their own (`Undefined`): they go through the dispatcher.
    fn call_target(&self, entry: &timers::TimerEntry) -> (Value, Vec<Value>) {
        let dispatcher = self.host_timers.0.borrow().dispatcher.clone();
        match (&entry.callback, dispatcher) {
            (Value::Undefined, Some(dispatcher)) => {
                (dispatcher, vec![Value::Number(entry.id.0 as f64)])
            }
            _ => (entry.callback.clone(), Vec::new()),
        }
    }

    fn apply_host_timer_commands(&mut self) {
        let commands = std::mem::take(&mut self.host_timers.0.borrow_mut().commands);
        for command in commands {
            match command {
                TimerCommand::Set {
                    id,
                    at,
                    delay,
                    repeat,
                } => {
                    self.timers.add(id, at, delay, repeat, Value::Undefined);
                }
                TimerCommand::Clear(id) => self.clear_timer(id),
                TimerCommand::SetRef(id, refd) => self.set_ref(id, refd),
            }
        }
    }

    fn release(&mut self, callback: &Value) {
        if let Value::Object(handle) = callback {
            self.isolate.release(*handle);
        }
    }

    fn record_lag(&mut self, due: Instant) {
        let lag = due.elapsed().as_secs_f64() * 1000.0;
        if self.lags_ms.len() == LAG_SAMPLES {
            self.lags_ms.pop_front();
        }
        self.lags_ms.push_back(lag);
        self.lag_max_ms = self.lag_max_ms.max(lag);
    }

    fn apply(&mut self, completion: Completion) {
        match completion.result {
            Ok(value) => {
                let value = self.materialize(value);
                self.isolate.resolve(completion.resolver, value);
            }
            Err(err) => {
                let reason = self.isolate.create_velyr_error(&err);
                self.isolate.reject(completion.resolver, reason);
            }
        }
    }

    fn materialize(&mut self, value: CompletionValue) -> Value {
        match value {
            CompletionValue::Undefined => Value::Undefined,
            CompletionValue::Null => Value::Null,
            CompletionValue::Bool(b) => Value::Bool(b),
            CompletionValue::Number(n) => Value::Number(n),
            CompletionValue::String(s) => Value::String(JsString::from(s.as_str())),
            CompletionValue::Bytes(b) => self.isolate.create_array_buffer(&b),
        }
    }
}

async fn sleep_until_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests;

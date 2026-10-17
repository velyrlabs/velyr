//! Isolate workers (ADR-010, ADR-014, `docs/specs/worker-api.md`).
//!
//! Each worker runs an ES module on its own thread, Tokio `current_thread` runtime, isolate
//! and event loop (`velyr_runtime_core::run_module_file_on_current_thread`). Its authority is
//! `parent ∩ requested ∩ deployment_policy`, it holds one `workers` quota unit of its parent
//! until its thread exits, its heap never exceeds the parent's, and it talks to the parent through
//! [`SerializedValue`] channels (see [`worker_host_functions`] for the worker side).
//!
//! Termination follows the spec order: new worker ops fail and the shutdown token is
//! cancelled, running JS is interrupted, the event loop stops and discards late completions,
//! the op context closes every resource, then the engine isolate is destroyed.

use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use velyr_engine_api::{Engine, HostFunctionDef, Isolate, SerializedValue, TerminationHandle};
use velyr_runtime_core::{LoopIsolate, RunError, RuntimeConfig, run_module_file_on_current_thread};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::{CompletionSender, TimerHandle};
use velyr_runtime_ops::{OpContext, take_leak_report};
use velyr_runtime_permissions::CapabilitySet;
use velyr_runtime_resources::{QuotaClass, QuotaContext};

pub mod auto;
mod channel;
mod port;
mod report;
pub mod shutdown;
pub mod supervisor;
mod web_api;

use channel::{Endpoint, Fill, Inbox, Packet, Tx};
pub use port::worker_host_functions;
use port::{WorkerPort, request_exit};
use report::ErrorReport;
pub use web_api::{
    MESSAGING_JS, SpawnFn, SpawnRequest, WORKER_JS, WorkerHost, WorkerScope, install_web_api,
    install_web_api_slots, start_web_api, web_api_host_functions,
};

/// Isolate ids handed to worker op contexts; 0 is the host's main isolate.
static NEXT_WORKER_ID: AtomicU64 = AtomicU64::new(1);

/// What a host's `process.exit` calls to end the worker it runs in.
pub type ExitHook = Arc<dyn Fn(i32) + Send + Sync>;

thread_local! {
    static EXIT_HOOK: RefCell<Option<ExitHook>> = const { RefCell::new(None) };
}

/// Ends the calling worker with `code`: the parent sees it in the `ended` event. Only meaningful
/// inside the `install` closure of [`WorkerHandle::spawn`] (it runs on the isolate's thread);
/// `None` anywhere else, including on a main isolate.
pub fn worker_exit_hook() -> Option<ExitHook> {
    EXIT_HOOK.with(|hook| hook.borrow().clone())
}

/// What a worker inherits from the isolate that spawns it.
pub struct ParentContext {
    pub capabilities: Arc<CapabilitySet>,
    pub deployment_policy: Arc<CapabilitySet>,
    /// Shared with the parent: every worker reserves one `workers` unit here.
    pub quotas: Arc<QuotaContext>,
    pub heap_limit_bytes: Option<usize>,
}

pub struct WorkerOptions {
    /// ES module entry point.
    pub entry: PathBuf,
    pub requested: Arc<CapabilitySet>,
    /// `None` inherits the parent's limit.
    pub heap_limit_bytes: Option<usize>,
    /// Base runtime config (platform init, the worker's own quotas, leak check, ...);
    /// capabilities, heap limit, shutdown token and leak collector are set by `spawn`.
    pub config: RuntimeConfig,
    /// Host functions besides [`worker_host_functions`], which `spawn` always adds.
    pub host_functions: Vec<HostFunctionDef>,
    /// Expected to capture and delete the ops it needs, the worker ops included.
    pub bootstrap_js: &'static str,
}

/// Parent-side handle. Dropping it terminates the worker without waiting for its thread.
pub struct WorkerHandle {
    capabilities: Arc<CapabilitySet>,
    shutdown: CancellationToken,
    termination: Receiver<TerminationHandle>,
    to_worker: Tx,
    from_worker: Inbox,
    result: Receiver<Result<(), RunError>>,
    thread: Option<JoinHandle<()>>,
    heap_limit_bytes: Option<usize>,
}

impl WorkerHandle {
    /// Reserves the quota, derives the authority and starts the worker thread. `install`
    /// runs on the worker isolate after the op context and the worker port are in place.
    pub fn spawn<E, F>(
        engine: E,
        parent: &ParentContext,
        options: WorkerOptions,
        install: F,
    ) -> Result<Self, VelyrError>
    where
        E: Engine + Send + 'static,
        F: FnOnce(&mut LoopIsolate<E::Isolate>, CompletionSender, TimerHandle) + Send + 'static,
    {
        let quota = parent.quotas.reserve(QuotaClass::Workers, 1)?;
        let capabilities = parent
            .capabilities
            .derive_child(&options.requested, &parent.deployment_policy);
        let shutdown = CancellationToken::new();
        let (to_worker, inbox) = channel::channel();
        let (outbox, from_worker) = channel::channel();
        let exit_outbox = outbox.clone();
        let (termination_tx, termination) = mpsc::channel();
        let (result_tx, result) = mpsc::channel();

        // Own limits, but `processes` is drawn from the parent's pool: one limit per process.
        let quotas = Arc::new(QuotaContext::child_of(
            options.config.quotas.clone().unwrap_or_default(),
            &parent.quotas,
        ));
        let heap_limit_bytes = capped_heap(options.heap_limit_bytes, parent.heap_limit_bytes);
        let port = WorkerPort {
            inbox,
            outbox,
            shutdown: shutdown.clone(),
            exit: Arc::default(),
        };
        let (worker_caps, token) = (capabilities.clone(), shutdown.clone());
        let exit = port.exit.clone();
        let setup = move |isolate: &mut LoopIsolate<E::Isolate>,
                          sender: CompletionSender,
                          timers: TimerHandle| {
            let id = NEXT_WORKER_ID.fetch_add(1, Ordering::Relaxed);
            isolate.set_slot(OpContext::new(id, sender.clone(), worker_caps, quotas));
            let (exit, shutdown) = (port.exit.clone(), port.shutdown.clone());
            isolate.set_slot(port);
            let handle = isolate.termination_handle();
            let stopper = handle.clone();
            EXIT_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Arc::new(move |code| {
                    request_exit(&exit, code);
                    shutdown.cancel();
                    stopper.terminate();
                }));
            });
            // Sent before the check: either the parent receives it or we see the cancel.
            let _ = termination_tx.send(handle.clone());
            if token.is_cancelled() {
                handle.terminate();
            }
            install(isolate, sender, timers);
        };

        let config = RuntimeConfig {
            capabilities: Some(capabilities.clone()),
            heap_limit_bytes,
            shutdown: Some(shutdown.clone()),
            leak_collector: Some(take_leak_report),
            ..options.config
        };
        let mut host_functions = options.host_functions;
        host_functions.extend(worker_host_functions());
        let (bootstrap_js, entry) = (options.bootstrap_js, options.entry);
        let thread = thread::Builder::new()
            .name("velyr-worker".into())
            .spawn(move || {
                let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
                    run_module_file_on_current_thread(
                        engine,
                        config,
                        host_functions,
                        bootstrap_js,
                        setup,
                        entry,
                    )
                }))
                .unwrap_or_else(|_| Err(RunError::Setup("isolate thread panicked".into())));
                // Free the slot before reporting, so a joined worker never still holds it.
                drop(quota);
                // Ordered after every message the worker posted: the parent's `error` event,
                // then `ended`. An exit the worker asked for is not an error.
                let requested = *exit.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(report) = failure_report(&outcome).filter(|_| requested.is_none()) {
                    let _ = exit_outbox.send(Packet::Error(report));
                }
                let code = requested.unwrap_or(i32::from(outcome.is_err()));
                let _ = exit_outbox.send(Packet::Ended(code));
                drop(exit_outbox);
                let _ = result_tx.send(outcome);
            })
            .map_err(|err| VelyrError::internal(format!("worker thread: {err}")))?;

        Ok(Self {
            capabilities,
            shutdown,
            termination,
            to_worker,
            from_worker,
            result,
            thread: Some(thread),
            heap_limit_bytes,
        })
    }

    /// Effective authority: `parent ∩ requested ∩ deployment_policy`.
    pub fn capabilities(&self) -> &Arc<CapabilitySet> {
        &self.capabilities
    }

    /// Heap limit the worker isolate runs with.
    pub fn heap_limit_bytes(&self) -> Option<usize> {
        self.heap_limit_bytes
    }

    /// Queues `message` for the worker's `op_worker_recv_message`.
    pub fn post_message(&self, message: SerializedValue) -> Result<(), VelyrError> {
        if self.shutdown.is_cancelled() {
            return Err(VelyrError::cancelled());
        }
        self.to_worker.send(Packet::plain(message))
    }

    /// Next message the worker posted; `None` once the worker is gone.
    pub async fn recv_message(&mut self) -> Option<SerializedValue> {
        while self.from_worker.fill().await == Fill::Ready {
            if let Some(Packet::Message(message)) = self.from_worker.pop() {
                return Some(message.value);
            }
        }
        None
    }

    /// The parent's side of the worker channel, for the isolate that owns this handle.
    pub(crate) fn endpoint(&self) -> Endpoint {
        Endpoint::direct(
            self.to_worker.clone(),
            self.from_worker.clone(),
            Some(self.shutdown.clone()),
        )
    }

    /// Stops the worker and waits up to `timeout` for it; a stopped worker reports
    /// [`RunError::Terminated`] (or the error it hit first).
    pub fn terminate(mut self, timeout: Duration) -> Result<(), RunError> {
        self.stop();
        self.wait(timeout)
    }

    /// Waits up to `timeout` for the worker to finish on its own.
    pub fn join(mut self, timeout: Duration) -> Result<(), RunError> {
        self.wait(timeout)
    }

    pub(crate) fn stop(&self) {
        self.shutdown.cancel();
        if let Ok(handle) = self.termination.try_recv() {
            handle.terminate();
        }
    }

    fn wait(&mut self, timeout: Duration) -> Result<(), RunError> {
        let outcome = self
            .result
            .recv_timeout(timeout)
            .map_err(|_| RunError::Setup("worker did not finish in time".into()))?;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        outcome
    }
}

/// What the parent's `error` event reports; a requested termination is not an error.
fn failure_report(outcome: &Result<(), RunError>) -> Option<ErrorReport> {
    match outcome {
        Ok(()) | Err(RunError::Terminated) => None,
        Err(RunError::Uncaught(exception)) => Some(ErrorReport::new(
            exception.message.clone(),
            exception.stack.as_deref(),
        )),
        Err(RunError::UnhandledRejection(reason)) => Some(ErrorReport::new(reason.clone(), None)),
        Err(RunError::QuotaExceeded { class }) => Some(ErrorReport::quota(
            RunError::QuotaExceeded { class }.to_string(),
            class,
        )),
        Err(other) => Some(ErrorReport::new(other.to_string(), None)),
    }
}

/// A worker never gets more heap than its parent; a larger request is capped, not refused.
fn capped_heap(requested: Option<usize>, parent: Option<usize>) -> Option<usize> {
    match (requested, parent) {
        (Some(requested), Some(parent)) => Some(requested.min(parent)),
        (requested, parent) => requested.or(parent),
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.stop();
        }
    }
}

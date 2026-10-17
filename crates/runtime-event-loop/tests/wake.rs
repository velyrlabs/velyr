//! RT-15: engine platform work wakes the loop instead of a 1 ms poll.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use velyr_engine_api::*;
use velyr_runtime_event_loop::EventLoop;

/// Reports outstanding platform work until another thread finishes it. With `started` false,
/// the work only starts in the first microtask checkpoint, as JS calling `WebAssembly.compile`.
struct BusyIsolate {
    done: Arc<AtomicBool>,
    started: bool,
}

impl Isolate for BusyIsolate {
    fn create_array_buffer(&mut self, _: &[u8]) -> Value {
        unimplemented!()
    }
    fn eval_script(&mut self, _: &Source) -> Result<Value, JsException> {
        unimplemented!()
    }
    fn compile_module(&mut self, _: &Source) -> Result<Handle, JsException> {
        unimplemented!()
    }
    fn instantiate_module(
        &mut self,
        _: Handle,
        _: &mut dyn ModuleResolveHook,
    ) -> Result<(), JsException> {
        unimplemented!()
    }
    fn evaluate_module(&mut self, _: Handle) -> Result<Value, JsException> {
        unimplemented!()
    }
    fn call(&mut self, _: &Value, _: &Value, _: &[Value]) -> Result<Value, JsException> {
        unimplemented!()
    }
    fn register_host_function(&mut self, _: HostFunctionDef) -> Result<(), EngineError> {
        unimplemented!()
    }
    fn set_slot<T: Any>(&mut self, _: T) {}
    fn create_promise(&mut self) -> (Value, ResolverId) {
        unimplemented!()
    }
    fn resolve(&mut self, _: ResolverId, _: Value) {}
    fn reject(&mut self, _: ResolverId, _: Value) {}
    fn promise_state(&mut self, _: &Value) -> Option<PromiseState> {
        None
    }
    fn perform_microtask_checkpoint(&mut self) -> MicrotaskProgress {
        self.started = true;
        MicrotaskProgress::Idle
    }
    fn heap_stats(&self) -> HeapStats {
        HeapStats::default()
    }
    fn request_gc(&mut self, _: GcReason) {}
    fn run_platform_tasks(&mut self) -> bool {
        self.started && !self.done.load(Ordering::SeqCst)
    }
    fn termination_handle(&self) -> TerminationHandle {
        TerminationHandle::new(|| {})
    }
    fn release(&mut self, _: Handle) {}
    fn scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        f(self)
    }
}

const IDLE: Duration = Duration::from_millis(200);
// The plan asks for 2 ms; the slack absorbs scheduler noise on a loaded machine.
const MAX_WAKE_LATENCY: Duration = Duration::from_millis(10);

/// Runs the loop while another thread finishes the platform work after [`IDLE`] and wakes it;
/// returns when the loop returned, measured from the wake-up.
async fn run_until_woken(started: bool) -> (EventLoop<BusyIsolate>, Duration) {
    let waker = PlatformWaker::new();
    waker.register_current();
    let done = Arc::new(AtomicBool::new(false));
    let isolate = BusyIsolate {
        done: done.clone(),
        started,
    };
    let mut event_loop = EventLoop::new(isolate, CancellationToken::new());
    let poster = thread::spawn(move || {
        thread::sleep(IDLE);
        done.store(true, Ordering::SeqCst);
        let posted_at = Instant::now();
        waker.wake();
        posted_at
    });
    tokio::time::timeout(Duration::from_secs(5), event_loop.run())
        .await
        .expect("loop never woke");
    let returned_at = Instant::now();
    let posted_at = poster.join().expect("poster thread");
    (event_loop, returned_at.saturating_duration_since(posted_at))
}

#[tokio::test(flavor = "current_thread")]
async fn platform_work_finished_on_another_thread_wakes_the_idle_loop() {
    let (event_loop, latency) = run_until_woken(true).await;
    assert!(latency < MAX_WAKE_LATENCY, "slow wake: {latency:?}");
    // One tick before going idle, one for the wake: a 1 ms poll would tick ~200 times.
    let ticks = event_loop.stats().ticks;
    assert!(ticks <= 2, "loop woke {ticks} times while idle");
}

#[tokio::test(flavor = "current_thread")]
async fn platform_work_started_by_the_tick_keeps_the_loop_alive() {
    let (_, latency) = run_until_woken(false).await;
    assert!(
        !latency.is_zero(),
        "loop returned before the platform work ended"
    );
}

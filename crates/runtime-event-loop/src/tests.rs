use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::thread::{self, ThreadId};
use std::time::Duration;

use velyr_engine_api::*;
use velyr_runtime_errors::VelyrError;

use super::*;

#[derive(Debug, Clone, PartialEq)]
enum Event {
    Checkpoint,
    Resolve(u64, ThreadId, Value),
    Reject(u64, Value),
    Buffer(Vec<u8>),
    Call(Value, Instant),
}

type Log = Rc<RefCell<Vec<Event>>>;

struct MockIsolate(Log, Arc<AtomicUsize>);

impl Isolate for MockIsolate {
    fn create_array_buffer(&mut self, bytes: &[u8]) -> Value {
        self.0.borrow_mut().push(Event::Buffer(bytes.to_vec()));
        Value::Null
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
    fn call(&mut self, func: &Value, _: &Value, _: &[Value]) -> Result<Value, JsException> {
        self.0
            .borrow_mut()
            .push(Event::Call(func.clone(), Instant::now()));
        Ok(Value::Undefined)
    }
    fn register_host_function(&mut self, _: HostFunctionDef) -> Result<(), EngineError> {
        unimplemented!()
    }
    fn set_slot<T: Any>(&mut self, _: T) {}
    fn create_promise(&mut self) -> (Value, ResolverId) {
        unimplemented!()
    }
    fn resolve(&mut self, resolver: ResolverId, value: Value) {
        let event = Event::Resolve(resolver.0, thread::current().id(), value);
        self.0.borrow_mut().push(event);
    }
    fn reject(&mut self, resolver: ResolverId, reason: Value) {
        self.0.borrow_mut().push(Event::Reject(resolver.0, reason));
    }
    fn promise_state(&mut self, _: &Value) -> Option<PromiseState> {
        None
    }
    fn perform_microtask_checkpoint(&mut self) -> MicrotaskProgress {
        self.0.borrow_mut().push(Event::Checkpoint);
        MicrotaskProgress::Idle
    }
    fn heap_stats(&self) -> HeapStats {
        HeapStats::default()
    }
    fn request_gc(&mut self, _: GcReason) {}
    fn termination_handle(&self) -> TerminationHandle {
        let terminations = self.1.clone();
        TerminationHandle::new(move || {
            terminations.fetch_add(1, Ordering::SeqCst);
        })
    }
    fn release(&mut self, _: Handle) {}
    fn scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        f(self)
    }
}

fn new_loop() -> (EventLoop<MockIsolate>, Log, CancellationToken) {
    let log = Log::default();
    let shutdown = CancellationToken::new();
    (
        EventLoop::new(MockIsolate(log.clone(), Arc::default()), shutdown.clone()),
        log,
        shutdown,
    )
}

fn ok(id: u64, value: CompletionValue) -> Completion {
    Completion {
        resolver: ResolverId(id),
        result: Ok(value),
    }
}

fn resolves(log: &Log) -> usize {
    log.borrow()
        .iter()
        .filter(|e| matches!(e, Event::Resolve(..)))
        .count()
}

#[test]
fn tick_processes_at_most_1024_and_leaves_the_rest() {
    let (mut el, log, _) = new_loop();
    let sender = el.sender();
    for id in 0..3000 {
        sender.begin_op().send(ok(id, CompletionValue::Undefined));
    }
    assert_eq!(el.tick(), 1024);
    assert_eq!(resolves(&log), 1024);
    assert_eq!(el.stats().pending_ops, 3000 - 1024);
    assert_eq!(el.tick(), 1024);
    assert_eq!(el.tick(), 952);
    assert_eq!(el.tick(), 0);
    assert_eq!(el.stats().ticks, 4);
}

#[test]
fn tick_brackets_completions_with_microtask_checkpoints() {
    let (mut el, log, _) = new_loop();
    let sender = el.sender();
    sender.begin_op().send(ok(1, CompletionValue::Number(2.0)));
    el.tick();
    let events = log.borrow();
    assert_eq!(events.first(), Some(&Event::Checkpoint));
    assert!(matches!(events[1], Event::Resolve(1, _, Value::Number(n)) if n == 2.0));
    assert_eq!(events.last(), Some(&Event::Checkpoint));
    assert_eq!(events.len(), 3);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn run_drains_fairly_then_terminates() {
    let (el, log, _) = new_loop();
    let mut el = el.with_max_completions_per_tick(10);
    let sender = el.sender();
    for id in 0..25 {
        sender.begin_op().send(ok(id, CompletionValue::Undefined));
    }
    el.run().await;
    assert_eq!(resolves(&log), 25);
    assert_eq!(el.stats().ticks, 3);
    assert_eq!(el.stats().pending_ops, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn completion_from_another_thread_is_applied_on_the_loop_thread() {
    let (mut el, log, _) = new_loop();
    let sender = el.sender();
    let op = sender.begin_op();
    let worker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        op.send(ok(7, CompletionValue::String("done".into())));
    });
    el.run().await;
    worker.join().unwrap();
    let events = log.borrow();
    let applied = events.iter().find_map(|e| match e {
        Event::Resolve(7, id, value) => Some((*id, value.clone())),
        _ => None,
    });
    assert_eq!(
        applied,
        Some((thread::current().id(), Value::String("done".into())))
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn run_ends_immediately_without_pending_ops() {
    let (mut el, _, _) = new_loop();
    el.run().await;
    assert_eq!(el.stats().ticks, 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn shutdown_stops_a_loop_waiting_on_pending_ops() {
    let (mut el, _, shutdown) = new_loop();
    let _op = el.sender().begin_op();
    shutdown.cancel();
    el.run().await;
    assert_eq!(el.stats().pending_ops, 1);
}

#[test]
fn errors_reject_and_bytes_become_buffers() {
    let (mut el, log, _) = new_loop();
    let sender = el.sender();
    sender.begin_op().send(Completion {
        resolver: ResolverId(1),
        result: Err(VelyrError::cancelled()),
    });
    sender
        .begin_op()
        .send(ok(2, CompletionValue::Bytes(vec![1, 2, 3])));
    el.tick();
    let events = log.borrow();
    assert!(events.contains(&Event::Reject(
        1,
        Value::String("VELYR_CANCELLED: operation cancelled".into())
    )));
    assert!(events.contains(&Event::Buffer(vec![1, 2, 3])));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn lag_measures_delay_of_a_rescheduled_tick() {
    let (el, _, _) = new_loop();
    let mut el = el.with_max_completions_per_tick(1);
    let sender = el.sender();
    for id in 0..2 {
        sender.begin_op().send(ok(id, CompletionValue::Undefined));
    }
    let delay = tokio::spawn(tokio::time::advance(Duration::from_millis(50)));
    tokio::join!(el.run(), async { delay.await.unwrap() });
    assert_eq!(el.stats().lag_max_ms, 50.0);
}

fn n(label: u32) -> Value {
    Value::Number(label.into())
}

fn calls(log: &Log) -> Vec<Value> {
    log.borrow()
        .iter()
        .filter_map(|e| match e {
            Event::Call(value, _) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

const MS: Duration = Duration::from_millis(1);

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn timers_fire_by_deadline_then_creation_order() {
    let (mut el, log, _) = new_loop();
    el.set_timer(10 * MS, false, n(1));
    el.set_timer(Duration::ZERO, false, n(2));
    el.set_timer(10 * MS, false, n(3));
    el.run().await;
    assert_eq!(calls(&log), vec![n(2), n(1), n(3)]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn unref_timer_alone_does_not_keep_the_loop_alive() {
    let (mut el, log, _) = new_loop();
    let id = el.set_timer(10 * MS, false, n(1));
    el.set_ref(id, false);
    el.run().await;
    assert!(calls(&log).is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn unref_timer_still_fires_while_a_ref_timer_keeps_the_loop_alive() {
    let (mut el, log, _) = new_loop();
    let id = el.set_timer(5 * MS, false, n(1));
    el.set_ref(id, false);
    el.set_timer(10 * MS, false, n(2));
    el.run().await;
    assert_eq!(calls(&log), vec![n(1), n(2)]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn microtasks_are_checkpointed_after_each_timer_callback() {
    let (mut el, log, _) = new_loop();
    el.set_timer(Duration::ZERO, false, n(1));
    el.set_timer(Duration::ZERO, false, n(2));
    el.run().await;
    let kinds: Vec<_> = log
        .borrow()
        .iter()
        .filter(|e| matches!(e, Event::Call(..) | Event::Checkpoint))
        .map(|e| matches!(e, Event::Call(..)))
        .collect();
    // checkpoint, call, checkpoint, call, checkpoint, checkpoint
    assert_eq!(kinds, [false, true, false, true, false, false]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn nesting_deeper_than_five_clamps_to_four_ms() {
    let (mut el, log, _) = new_loop();
    let id = el.set_timer(Duration::ZERO, true, n(1));
    for _ in 0..5 {
        el.tick();
    }
    assert_eq!(calls(&log).len(), 5);
    el.tick();
    tokio::time::advance(3 * MS).await;
    el.tick();
    assert_eq!(calls(&log).len(), 5);
    tokio::time::advance(MS).await;
    el.tick();
    assert_eq!(calls(&log).len(), 6);
    el.clear_timer(id);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn clear_timer_cancels_a_pending_timer() {
    let (mut el, log, _) = new_loop();
    let id = el.set_timer(10 * MS, false, n(1));
    el.clear_timer(id);
    el.run().await;
    assert!(calls(&log).is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn interval_reschedules_after_each_callback_until_cleared() {
    let (mut el, log, _) = new_loop();
    let id = el.set_timer(10 * MS, true, n(1));
    for expected in 1..=3 {
        tokio::time::advance(10 * MS).await;
        el.tick();
        assert_eq!(calls(&log).len(), expected);
    }
    el.clear_timer(id);
    tokio::time::advance(10 * MS).await;
    el.tick();
    assert_eq!(calls(&log).len(), 3);
    el.run().await;
}

#[test]
fn clearing_a_running_interval_prevents_its_reschedule() {
    let mut timers = crate::timers::Timers::default();
    let now = Instant::now();
    let id = timers.add(TimerId(1), now, 10 * MS, true, n(1));
    let entry = timers.begin_run(id).unwrap();
    timers.clear(id);
    assert_eq!(timers.end_run(now, entry), Some(n(1)));
    assert!(timers.next_deadline().is_none());
}

fn new_loop_counting_terminations() -> (EventLoop<MockIsolate>, Log, Arc<AtomicUsize>) {
    let log = Log::default();
    let terminations = Arc::new(AtomicUsize::new(0));
    let isolate = MockIsolate(log.clone(), terminations.clone());
    (
        EventLoop::new(isolate, CancellationToken::new()),
        log,
        terminations,
    )
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn late_completion_after_terminate_and_drop_is_ignored() {
    let (mut el, log, terminations) = new_loop_counting_terminations();
    let op = el.sender().begin_op();
    let late = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        op.send(ok(1, CompletionValue::Undefined));
    });
    el.terminate();
    assert_eq!(terminations.load(Ordering::SeqCst), 1);
    drop(el);
    let events_at_drop = log.borrow().len();
    late.await.unwrap();
    assert_eq!(log.borrow().len(), events_at_drop);
}

#[test]
fn terminate_discards_queued_completions_and_new_ops_start_cancelled() {
    let (mut el, log, _) = new_loop_counting_terminations();
    let sender = el.sender();
    sender.begin_op().send(ok(1, CompletionValue::Undefined));
    el.terminate();
    assert_eq!(el.tick(), 0);
    assert_eq!(resolves(&log), 0);
    assert_eq!(el.stats().pending_ops, 0);
    let op = sender.begin_op();
    assert!(op.token().is_cancelled());
    op.send(ok(2, CompletionValue::Undefined));
    assert_eq!(el.tick(), 0);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn op_observes_cancellation_within_10ms() {
    let (mut el, _, _) = new_loop_counting_terminations();
    let token = el.sender().begin_op().token();
    let watcher = tokio::spawn(async move {
        token.cancelled().await;
        Instant::now()
    });
    tokio::task::yield_now().await;
    let cancelled_at = Instant::now();
    el.terminate();
    let observed_at = watcher.await.unwrap();
    assert!(observed_at - cancelled_at < Duration::from_millis(10));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn dropping_an_op_guard_without_sending_releases_the_loop() {
    let (mut el, _, _) = new_loop_counting_terminations();
    drop(el.sender().begin_op());
    el.run().await;
    assert_eq!(el.stats().pending_ops, 0);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn shutdown_drains_cooperative_ops_then_terminates() {
    let (el, log, terminations) = new_loop_counting_terminations();
    let mut el = el.with_shutdown_deadline(Duration::from_secs(1));
    let op = el.sender().begin_op();
    tokio::spawn(async move {
        op.token().cancelled().await;
        op.send(Completion {
            resolver: ResolverId(1),
            result: Err(VelyrError::cancelled()),
        });
    });
    el.shutdown().await;
    assert!(
        log.borrow()
            .iter()
            .any(|e| matches!(e, Event::Reject(1, _)))
    );
    assert_eq!(terminations.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn shutdown_forces_termination_after_the_deadline() {
    let (el, _, terminations) = new_loop_counting_terminations();
    let mut el = el.with_shutdown_deadline(Duration::from_millis(200));
    let _stuck = el.sender().begin_op();
    let start = Instant::now();
    el.shutdown().await;
    assert_eq!(start.elapsed(), Duration::from_millis(200));
    assert_eq!(terminations.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn host_timers_run_through_the_dispatcher_and_keep_the_loop_alive() {
    let (mut el, log, _) = new_loop();
    let handle = el.timer_handle();
    handle.set_dispatcher(n(99));
    let first = handle.set(5 * MS, false);
    let second = handle.set(Duration::ZERO, false);
    assert_eq!((first, second), (TimerId(1), TimerId(2)));
    el.run().await;
    assert_eq!(calls(&log), vec![n(99), n(99)]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn host_timer_cleared_before_the_first_tick_never_fires() {
    let (mut el, log, _) = new_loop();
    let handle = el.timer_handle();
    handle.set_dispatcher(n(99));
    let id = handle.set(Duration::ZERO, false);
    handle.clear(id);
    el.run().await;
    assert!(calls(&log).is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn host_unref_timer_does_not_keep_the_loop_alive() {
    let (mut el, log, _) = new_loop();
    let handle = el.timer_handle();
    handle.set_dispatcher(n(99));
    let id = handle.set(5 * MS, true);
    handle.set_ref(id, false);
    el.run().await;
    assert!(calls(&log).is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn host_and_rust_timers_share_one_id_space() {
    let (mut el, _, _) = new_loop();
    let handle = el.timer_handle();
    let rust = el.set_timer(MS, false, n(1));
    assert_eq!(handle.set(MS, false), TimerId(rust.0 + 1));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn immediates_run_before_due_timers_in_the_same_tick() {
    let (mut el, log, _) = new_loop();
    let handle = el.timer_handle();
    let immediates = handle.immediates();
    immediates.set_dispatcher(n(98));
    handle.set_dispatcher(n(99));
    handle.set(Duration::ZERO, false);
    immediates.set();
    el.run().await;
    assert_eq!(calls(&log), vec![n(98), n(99)]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cleared_immediate_never_runs_and_ids_are_sequential() {
    let (mut el, log, _) = new_loop();
    let immediates = el.timer_handle().immediates();
    immediates.set_dispatcher(n(98));
    let first = immediates.set();
    let second = immediates.set();
    assert_eq!((first, second), (ImmediateId(1), ImmediateId(2)));
    immediates.clear(first);
    el.run().await;
    assert_eq!(calls(&log), vec![n(98)]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn unref_immediate_does_not_keep_the_loop_alive() {
    let (mut el, log, _) = new_loop();
    let immediates = el.timer_handle().immediates();
    immediates.set_dispatcher(n(98));
    let id = immediates.set();
    immediates.set_ref(id, false);
    // A ref'd timer far away: the unref'd immediate must not make the loop spin on it.
    el.timer_handle().set_dispatcher(n(99));
    el.timer_handle().set(50 * MS, false);
    el.run().await;
    assert_eq!(calls(&log), vec![n(98), n(99)]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn immediate_created_by_an_immediate_runs_on_the_next_tick() {
    let (mut el, log, _) = new_loop();
    let immediates = el.timer_handle().immediates();
    immediates.set_dispatcher(n(98));
    immediates.set();
    el.tick();
    assert_eq!(calls(&log), vec![n(98)]);
    // Simulate JS scheduling another one while running: it must wait for the next tick.
    immediates.set();
    assert_eq!(calls(&log).len(), 1);
    el.tick();
    assert_eq!(calls(&log).len(), 2);
}

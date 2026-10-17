//! RT-15: V8 foreground tasks wake the isolate thread's loop through its `PlatformWaker`, and
//! still run from `run_platform_tasks` (async wasm compiles, `FinalizationRegistry` cleanup).

use std::sync::mpsc;
use std::time::Duration;

use velyr_engine_api::{
    Engine, GcReason, Isolate, IsolateOptions, PlatformWaker, PromiseState, Source, Value,
};
use velyr_engine_v8::{V8Engine, V8Isolate};

const WAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// An isolate plus the wake-ups its platform tasks send.
fn new_isolate() -> (V8Isolate, mpsc::Receiver<()>) {
    let iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let (tx, rx) = mpsc::channel();
    let waker = PlatformWaker::current().expect("create_isolate registers a waker");
    waker.set_wake_fn(move || {
        let _ = tx.send(());
    });
    (iso, rx)
}

fn eval(iso: &mut V8Isolate, text: &str) -> Value {
    iso.eval_script(&Source {
        name: "wake.js".into(),
        text: text.into(),
    })
    .expect("eval")
}

/// Sleeps until a platform task wakes the thread, then runs it, like the event loop does.
fn run_after_wake(iso: &mut V8Isolate, wakes: &mpsc::Receiver<()>) {
    wakes
        .recv_timeout(WAKE_TIMEOUT)
        .expect("no platform wake-up");
    iso.run_platform_tasks();
    iso.perform_microtask_checkpoint();
}

#[test]
fn wasm_compile_settles_on_wake_ups_alone() {
    let (mut iso, wakes) = new_isolate();
    let promise = eval(
        &mut iso,
        "WebAssembly.compile(new Uint8Array([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]))
           .then(m => m instanceof WebAssembly.Module)",
    );
    while let Some(PromiseState::Pending) = iso.promise_state(&promise) {
        run_after_wake(&mut iso, &wakes);
    }
    match iso.promise_state(&promise) {
        Some(PromiseState::Fulfilled(value)) => assert_eq!(value, Value::Bool(true)),
        other => panic!("compile: {other:?}"),
    }
    assert!(
        !iso.run_platform_tasks(),
        "platform work left after the compile"
    );
}

#[test]
fn finalization_registry_callbacks_run_after_gc() {
    let (mut iso, wakes) = new_isolate();
    eval(
        &mut iso,
        "globalThis.finalized = false;
         globalThis.registry = new FinalizationRegistry(() => { globalThis.finalized = true; });
         (() => registry.register({}, 1))();",
    );
    iso.request_gc(GcReason::Idle);
    while eval(&mut iso, "finalized") != Value::Bool(true) {
        run_after_wake(&mut iso, &wakes);
    }
}

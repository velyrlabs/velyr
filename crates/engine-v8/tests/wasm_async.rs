//! Async WebAssembly compilation finishes on V8 platform tasks (VELYR-218): the isolate must
//! report the outstanding work and run it from `run_platform_tasks`, or the promises never settle.

use std::time::{Duration, Instant};

use velyr_engine_api::{Engine, Isolate, IsolateOptions, PromiseState, Source, Value};
use velyr_engine_v8::{V8Engine, V8Isolate};

const EMPTY_MODULE: &str = "new Uint8Array([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0])";

/// Drives `run_platform_tasks` and microtasks like the event loop does, until `promise` settles.
fn settle(iso: &mut V8Isolate, promise: &Value) -> PromiseState {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let busy = iso.run_platform_tasks();
        iso.perform_microtask_checkpoint();
        match iso.promise_state(promise) {
            Some(PromiseState::Pending) | None if Instant::now() < deadline => {
                assert!(busy, "promise pending but no platform work reported");
                std::thread::sleep(Duration::from_millis(1));
            }
            Some(state) => return state,
            None => panic!("not a promise"),
        }
    }
}

fn eval(iso: &mut V8Isolate, text: String) -> Value {
    iso.eval_script(&Source {
        name: "wasm.js".into(),
        text,
    })
    .expect("eval")
}

fn new_isolate() -> V8Isolate {
    V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

#[test]
fn compile_resolves_with_a_module() {
    let mut iso = new_isolate();
    let promise = eval(
        &mut iso,
        format!("WebAssembly.compile({EMPTY_MODULE}).then(m => m instanceof WebAssembly.Module)"),
    );
    match settle(&mut iso, &promise) {
        PromiseState::Fulfilled(value) => assert_eq!(value, Value::Bool(true)),
        other => panic!("compile: {other:?}"),
    }
}

#[test]
fn instantiate_resolves_with_module_and_instance() {
    let mut iso = new_isolate();
    let promise = eval(
        &mut iso,
        format!(
            "WebAssembly.instantiate({EMPTY_MODULE}).then(r =>
               r.module instanceof WebAssembly.Module && r.instance instanceof WebAssembly.Instance)"
        ),
    );
    match settle(&mut iso, &promise) {
        PromiseState::Fulfilled(value) => assert_eq!(value, Value::Bool(true)),
        other => panic!("instantiate: {other:?}"),
    }
}

#[test]
fn invalid_bytes_reject_and_the_platform_work_ends() {
    let mut iso = new_isolate();
    let promise = eval(
        &mut iso,
        "WebAssembly.compile(new Uint8Array([0, 1, 2, 3])).catch(e => e instanceof WebAssembly.CompileError)".into(),
    );
    match settle(&mut iso, &promise) {
        PromiseState::Fulfilled(value) => assert_eq!(value, Value::Bool(true)),
        other => panic!("compile: {other:?}"),
    }
    // The event loop exits once this reports no outstanding work.
    let idle = (0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(1));
        !iso.run_platform_tasks()
    });
    assert!(idle, "platform work still reported after the rejection");
}

#[test]
fn no_platform_work_is_reported_without_wasm() {
    let mut iso = new_isolate();
    eval(&mut iso, "Promise.resolve(1).then(x => x + 1)".into());
    iso.perform_microtask_checkpoint();
    assert!(!iso.run_platform_tasks());
}

use std::time::{Duration, Instant};

use velyr_engine_api::{
    Engine, EngineFeatures, GcReason, Isolate, IsolateOptions, JsBigInt, JsString,
    MicrotaskProgress, PromiseState, Source, Value,
};
use velyr_engine_v8::{V8Engine, V8Isolate};

fn isolate() -> V8Isolate {
    V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn src(text: &str) -> Source {
    Source {
        name: "test.js".into(),
        text: text.into(),
    }
}

fn eval(iso: &mut V8Isolate, text: &str) -> Value {
    iso.eval_script(&src(text)).expect("eval")
}

#[test]
fn init_platform_is_idempotent() {
    V8Engine::init_platform();
    V8Engine::init_platform();
}

#[test]
fn eval_number() {
    assert_eq!(eval(&mut isolate(), "20 + 22"), Value::Number(42.0));
}

#[test]
fn eval_primitives() {
    let mut iso = isolate();
    assert_eq!(eval(&mut iso, "undefined"), Value::Undefined);
    assert_eq!(eval(&mut iso, "null"), Value::Null);
    assert_eq!(eval(&mut iso, "true"), Value::Bool(true));
    assert_eq!(
        eval(&mut iso, "'h\\u{1F600}'"),
        Value::String(JsString::from("h\u{1F600}"))
    );
}

#[test]
fn eval_preserves_lone_surrogate() {
    let Value::String(s) = eval(&mut isolate(), "'\\uD800'") else {
        panic!("expected string");
    };
    assert_eq!(s.as_utf16(), &[0xD800]);
}

#[test]
fn eval_one_byte_and_two_byte_strings() {
    let mut iso = isolate();
    assert_eq!(
        eval(&mut iso, "'abc'"),
        Value::String(JsString::from("abc"))
    );
    assert_eq!(
        eval(&mut iso, "'\\u00e9\\u4e2d'"),
        Value::String(JsString::from("\u{e9}\u{4e2d}"))
    );
}

#[test]
fn eval_bigint_roundtrip() {
    let mut iso = isolate();
    assert_eq!(
        eval(&mut iso, "-258n"),
        Value::BigInt(JsBigInt::from_i64(-258))
    );
    assert_eq!(eval(&mut iso, "0n"), Value::BigInt(JsBigInt::from_i64(0)));
    let big = eval(&mut iso, "2n ** 100n");
    let Value::BigInt(b) = &big else {
        panic!("expected bigint")
    };
    assert_eq!(b.magnitude_le().len(), 13);
    assert_eq!(b.magnitude_le()[12], 0x10);
    // Value -> JS: pass the BigInt back through a function.
    let id = eval(&mut iso, "(x) => x === 2n ** 100n");
    let result = iso.call(&id, &Value::Undefined, &[big]).expect("call");
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn throw_type_error_maps_to_exception() {
    let err = isolate()
        .eval_script(&src("throw new TypeError('x')"))
        .unwrap_err();
    assert!(err.message.contains("TypeError: x"), "{}", err.message);
    assert!(err.stack.as_deref().is_some_and(|s| !s.is_empty()));
    assert!(err.value.is_some());
}

#[test]
fn syntax_error_maps_to_exception() {
    let err = isolate().eval_script(&src("let = ;")).unwrap_err();
    assert!(err.message.contains("SyntaxError"), "{}", err.message);
}

#[test]
fn throw_primitive_keeps_value() {
    let err = isolate().eval_script(&src("throw 7")).unwrap_err();
    assert_eq!(err.value, Some(Value::Number(7.0)));
}

#[test]
fn terminate_from_another_thread_and_recover() {
    let mut iso = isolate();
    let handle = iso.termination_handle();
    let killer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        handle.terminate();
    });
    let start = Instant::now();
    let err = iso.eval_script(&src("while(true){}")).unwrap_err();
    killer.join().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(err.message.contains("terminated"), "{}", err.message);
    assert_eq!(eval(&mut iso, "1 + 1"), Value::Number(2.0));
}

#[test]
fn stale_handle_is_rejected_after_slot_reuse() {
    let mut iso = isolate();
    let Value::Object(old) = eval(&mut iso, "(() => 1)") else {
        panic!("expected object");
    };
    iso.release(old);
    let Value::Object(new) = eval(&mut iso, "(() => 2)") else {
        panic!("expected object");
    };
    assert_eq!(new.id, old.id);
    assert_ne!(new.generation, old.generation);
    assert!(
        iso.call(&Value::Object(old), &Value::Undefined, &[])
            .is_err()
    );
    assert_eq!(
        iso.call(&Value::Object(new), &Value::Undefined, &[])
            .unwrap(),
        Value::Number(2.0)
    );
    iso.release(old); // already stale: ignored, must not free the new slot
    assert!(
        iso.call(&Value::Object(new), &Value::Undefined, &[])
            .is_ok()
    );
}

#[test]
fn scope_releases_handles_created_inside() {
    let mut iso = isolate();
    let leaked = iso.scope(|iso| {
        let Value::Object(h) = eval(iso, "(() => 1)") else {
            panic!("expected object");
        };
        assert!(iso.call(&Value::Object(h), &Value::Undefined, &[]).is_ok());
        h
    });
    assert!(
        iso.call(&Value::Object(leaked), &Value::Undefined, &[])
            .is_err()
    );
}

#[test]
fn call_passes_this_and_args() {
    let mut iso = isolate();
    let f = eval(&mut iso, "(function (a, b) { return this.k + a + b; })");
    let this = eval(&mut iso, "({ k: 1 })");
    let out = iso
        .call(&f, &this, &[Value::Number(2.0), Value::Number(3.0)])
        .unwrap();
    assert_eq!(out, Value::Number(6.0));
}

#[test]
fn promises_resolve_and_drain() {
    let mut iso = isolate();
    let (promise, resolver) = iso.create_promise();
    assert_eq!(iso.promise_state(&promise), Some(PromiseState::Pending));
    iso.resolve(resolver, Value::Number(5.0));
    iso.resolve(resolver, Value::Number(6.0)); // second settle is a no-op
    assert_eq!(
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Number(5.0)))
    );
    let (p2, r2) = iso.create_promise();
    iso.reject(r2, Value::Number(1.0));
    assert_eq!(
        iso.promise_state(&p2),
        Some(PromiseState::Rejected(Value::Number(1.0)))
    );
    assert_eq!(iso.promise_state(&Value::Number(1.0)), None);
}

#[test]
fn microtasks_run_only_on_explicit_checkpoint() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "globalThis.x = 0; Promise.resolve().then(() => { globalThis.x = 1; });",
    );
    assert_eq!(eval(&mut iso, "globalThis.x"), Value::Number(0.0));
    assert_eq!(
        iso.perform_microtask_checkpoint(),
        MicrotaskProgress::Progressed
    );
    assert_eq!(iso.perform_microtask_checkpoint(), MicrotaskProgress::Idle);
    assert_eq!(eval(&mut iso, "globalThis.x"), Value::Number(1.0));
}

#[test]
fn heap_stats_and_gc() {
    let mut iso = isolate();
    let stats = iso.heap_stats();
    assert!(stats.used_bytes > 0 && stats.total_bytes >= stats.used_bytes);
    iso.request_gc(GcReason::Explicit);
}

#[test]
fn heap_limit_option_is_reported() {
    let iso = V8Engine::new()
        .create_isolate(IsolateOptions {
            heap_limit_bytes: Some(64 * 1024 * 1024),
        })
        .unwrap();
    assert!(iso.heap_stats().limit_bytes.is_some());
}

#[test]
fn features_declare_implemented() {
    let f = V8Engine::new().features();
    assert!(
        f.contains(EngineFeatures::PROMISES | EngineFeatures::BIGINT | EngineFeatures::MODULES)
    );
}

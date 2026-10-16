//! `heap_limit_bytes` is a hard limit on the native isolate's GC heap (VELYR-182): a
//! script that allocates without end fails with the `heap` quota error instead of
//! taking the whole process down, and the host keeps running.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

const LIMIT_BYTES: usize = 32 << 20;

fn isolate() -> NativeIsolate {
    let options = IsolateOptions {
        heap_limit_bytes: Some(LIMIT_BYTES),
    };
    NativeEngine.create_isolate(options).expect("isolate")
}

fn eval(iso: &mut NativeIsolate, text: &str) -> Result<Value, String> {
    let source = Source {
        name: "heap_limit.js".into(),
        text: text.into(),
    };
    iso.eval_script(&source)
        .map_err(|error| error.quota_class().unwrap_or(&error.message).to_string())
}

#[test]
fn endless_object_allocation_fails_with_the_heap_quota() {
    let mut iso = isolate();
    let result = eval(&mut iso, "const kept = []; for (;;) kept.push({ n: kept.length });");
    assert_eq!(result.err().as_deref(), Some("heap"));
}

#[test]
fn endless_string_growth_fails_with_the_heap_quota() {
    let mut iso = isolate();
    let script = "const kept = []; for (let i = 0; ; i++) kept.push('x'.repeat(1000) + i);";
    assert_eq!(eval(&mut iso, script).err().as_deref(), Some("heap"));
}

#[test]
fn endless_array_growth_fails_with_the_heap_quota() {
    let mut iso = isolate();
    let script = "const kept = []; for (let i = 0; ; i++) kept.push(i, i, i, i);";
    assert_eq!(eval(&mut iso, script).err().as_deref(), Some("heap"));
}

#[test]
fn a_fresh_isolate_runs_after_one_hit_the_limit() {
    let mut iso = isolate();
    let script = "const kept = []; for (;;) kept.push([kept.length]);";
    assert!(eval(&mut iso, script).is_err());
    drop(iso);
    assert_eq!(eval(&mut isolate(), "1 + 1"), Ok(Value::Number(2.0)));
}

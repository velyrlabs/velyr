//! VELYR-137: exported object handles are GC roots until released, and an observed
//! termination is cleared so the isolate can run again.

use velyr_engine_api::*;
use velyr_engine_native::NativeEngine;

fn isolate() -> impl Isolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .unwrap()
}

fn eval(iso: &mut impl Isolate, text: &str) -> Value {
    let source = Source {
        name: "test.js".into(),
        text: text.into(),
    };
    iso.eval_script(&source)
        .unwrap_or_else(|e| panic!("{text}: {}", e.message))
}

/// Allocates well past the collection threshold, so the heap collects several times.
const CHURN: &str = "let g; for (let i = 0; i < 100000; i++) g = [i, { i }]; 0";

fn read_a(iso: &mut impl Isolate, object: &Value) -> Result<Value, JsException> {
    let getter = eval(iso, "(function () { return this.a; })");
    iso.call(&getter, object, &[])
}

#[test]
fn exported_object_survives_collections() {
    let mut iso = isolate();
    let object = eval(&mut iso, "({ a: 42 })");
    eval(&mut iso, CHURN);
    assert_eq!(read_a(&mut iso, &object).unwrap(), Value::Number(42.0));
}

#[test]
fn released_handle_is_stale() {
    let mut iso = isolate();
    let Value::Object(handle) = eval(&mut iso, "({ a: 1 })") else {
        panic!("expected an object handle");
    };
    iso.release(handle);
    iso.release(handle);
    let error = read_a(&mut iso, &Value::Object(handle)).unwrap_err();
    assert!(error.message.starts_with("TypeError:"), "{}", error.message);
}

#[test]
fn scope_releases_the_handles_created_inside() {
    let mut iso = isolate();
    let inner = iso.scope(|iso| {
        let object = eval(iso, "({ a: 7 })");
        assert_eq!(read_a(iso, &object).unwrap(), Value::Number(7.0));
        object
    });
    assert!(read_a(&mut iso, &inner).is_err());
}

#[test]
fn isolate_runs_again_after_an_observed_termination() {
    let mut iso = isolate();
    iso.termination_handle().terminate();
    let source = Source {
        name: "test.js".into(),
        text: "while (true) {}".into(),
    };
    let error = iso.eval_script(&source).unwrap_err();
    assert_eq!(error.message, "execution terminated");
    assert_eq!(eval(&mut iso, "1 + 1"), Value::Number(2.0));
}

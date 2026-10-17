//! `Array.fromAsync` on engine-native (N17).

use velyr_engine_api::{
    Engine, IsolateOptions, Isolate, JsString, MicrotaskProgress, Source, Value,
};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, text: &str) -> Value {
    let source = Source {
        name: "from_async.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(value) => value,
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

/// Runs `expr` (an `Array.fromAsync` call), drains the queue and returns the settled outcome.
fn settle(expr: &str) -> String {
    let mut iso = isolate();
    eval(
        &mut iso,
        &format!(
            "var out = 'pending';
             var p; try {{ p = {expr}; }} catch (e) {{ out = 'SYNC THROW ' + e; }}
             if (p) p.then(v => out = 'ok:' + (Array.isArray(v) ? v.join() : v.constructor.name + ':' + v.length),
                           e => out = 'err:' + e.constructor.name);"
        ),
    );
    while iso.perform_microtask_checkpoint() == MicrotaskProgress::Progressed {}
    match eval(&mut iso, "out") {
        Value::String(s) => s.to_string_lossy(),
        other => panic!("unexpected {other:?}"),
    }
}

fn text(iso: &mut NativeIsolate, expr: &str) -> String {
    match eval(iso, expr) {
        Value::String(s) => s.to_string_lossy(),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn awaits_sync_iterable_values() {
    assert_eq!(settle("Array.fromAsync([1, Promise.resolve(2)])"), "ok:1,2");
}

#[test]
fn maps_with_an_async_function() {
    assert_eq!(
        settle("Array.fromAsync([1, 2], async x => x * 2)"),
        "ok:2,4"
    );
}

#[test]
fn consumes_async_iterables() {
    assert_eq!(
        settle("Array.fromAsync((async function* () { yield 1; yield 2; })())"),
        "ok:1,2"
    );
}

#[test]
fn consumes_array_likes() {
    assert_eq!(settle("Array.fromAsync({ length: 2, 0: 'a', 1: 'b' })"), "ok:a,b");
}

#[test]
fn uses_this_as_constructor() {
    assert_eq!(
        settle("Array.fromAsync.call(function C() {}, [1])"),
        "ok:C:1"
    );
}

#[test]
fn rejects_instead_of_throwing_synchronously() {
    assert_eq!(settle("Array.fromAsync([1], 5)"), "err:TypeError");
    assert_eq!(settle("Array.fromAsync(null)"), "err:TypeError");
}

#[test]
fn has_standard_function_shape() {
    let mut iso = isolate();
    assert_eq!(
        text(
            &mut iso,
            "const f = Array.fromAsync; const d = Object.getOwnPropertyDescriptor(Array, 'fromAsync');
             [f.length, f.name, d.writable, d.enumerable, d.configurable,
              Object.hasOwn(f, 'prototype')].join()"
        ),
        "1,fromAsync,true,false,true,false"
    );
    let _ = JsString::from("");
}

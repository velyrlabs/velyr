//! `AsyncDisposableStack` (explicit resource management, N33).

use velyr_engine_api::{Engine, Isolate, IsolateOptions, MicrotaskProgress, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn eval(iso: &mut NativeIsolate, text: &str) -> String {
    let source = Source {
        name: "async_disposable_stack.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(Value::String(s)) => s.to_string_lossy(),
        Ok(Value::Bool(b)) => b.to_string(),
        Ok(Value::Number(n)) => n.to_string(),
        Ok(other) => panic!("`{text}` returned {other:?}"),
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

/// Runs `setup` (which pushes into `log`), drains the microtask queue and returns `String(log)`.
fn log_after(setup: &str) -> String {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    eval(&mut iso, &format!("var log = []; {setup}; 'ok'"));
    while iso.perform_microtask_checkpoint() == MicrotaskProgress::Progressed {}
    eval(&mut iso, "String(log)")
}

fn sync(text: &str) -> String {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    eval(&mut iso, text)
}

#[test]
fn disposes_in_reverse_order() {
    let source = "var s = new AsyncDisposableStack();
        s.defer(() => log.push(1)); s.adopt('r', v => log.push(v));
        s.use({ async [Symbol.asyncDispose]() { log.push(3); } });
        s.disposeAsync().then(() => log.push('done'))";
    assert_eq!(log_after(source), "3,r,1,done");
}

#[test]
fn awaits_each_resource_before_the_next() {
    let source = "var s = new AsyncDisposableStack();
        s.defer(() => log.push('second'));
        s.defer(() => Promise.resolve().then(() => Promise.resolve()).then(() => log.push('first')));
        s.disposeAsync()";
    assert_eq!(log_after(source), "first,second");
}

#[test]
fn use_falls_back_to_sync_dispose() {
    let source = "var s = new AsyncDisposableStack();
        var o = { [Symbol.dispose]() { log.push('sync'); return 5; } };
        log.push(s.use(o) === o);
        s.disposeAsync()";
    assert_eq!(log_after(source), "true,sync");
}

#[test]
fn second_dispose_resolves_undefined_without_rerun() {
    let source = "var s = new AsyncDisposableStack(); var n = 0; s.defer(() => n++);
        s.disposeAsync().then(() => s.disposeAsync()).then(v => log.push(n, v))";
    assert_eq!(log_after(source), "1,");
}

#[test]
fn empty_stack_still_awaits() {
    let source = "var s = new AsyncDisposableStack();
        s.disposeAsync().then(() => log.push('settled')); log.push('sync')";
    assert_eq!(log_after(source), "sync,settled");
}

#[test]
fn brand_check_rejects_instead_of_throwing() {
    let source = "var p = AsyncDisposableStack.prototype.disposeAsync.call({});
        p.catch(e => log.push(e.constructor.name))";
    assert_eq!(log_after(source), "TypeError");
}

#[test]
fn errors_aggregate_into_suppressed_error() {
    let source = "var s = new AsyncDisposableStack(), a = new Error('a'), b = new Error('b');
        s.defer(() => { throw a; }); s.defer(() => Promise.reject(b));
        s.disposeAsync().catch(e => log.push(e instanceof SuppressedError, e.error === a, e.suppressed === b))";
    assert_eq!(log_after(source), "true,true,true");
}

#[test]
fn use_validates_its_argument() {
    let source = "var s = new AsyncDisposableStack(), out = [];
        for (const v of [1, {}, { [Symbol.asyncDispose]: 1 }, { [Symbol.dispose]: 1 }]) {
          try { s.use(v); out.push('none'); } catch (e) { out.push(e.constructor.name); }
        }
        out.push(String(s.use(null)), String(s.use(undefined))); out.join()";
    assert_eq!(sync(source), "TypeError,TypeError,TypeError,TypeError,null,undefined");
}

#[test]
fn state_errors_and_move() {
    let source = "var s = new AsyncDisposableStack(), n = 0; s.defer(() => n++);
        var t = s.move(); var r = [t instanceof AsyncDisposableStack, s.disposed, t.disposed];
        try { s.defer(() => {}); } catch (e) { r.push(e.constructor.name); }
        r.join()";
    assert_eq!(sync(source), "true,true,false,ReferenceError");
}

#[test]
fn constructor_and_shape() {
    let source = "var P = AsyncDisposableStack.prototype, g = Object.getOwnPropertyDescriptor(P, 'disposed').get, r = [];
        try { AsyncDisposableStack(); } catch (e) { r.push(e.constructor.name); }
        r.push(AsyncDisposableStack.length, P[Symbol.toStringTag], P.disposeAsync.length, P.disposeAsync.name,
          P.disposeAsync === P[Symbol.asyncDispose], g.name, Object.keys(P).length,
          Object.getOwnPropertyDescriptor(AsyncDisposableStack, 'prototype').writable);
        r.join()";
    assert_eq!(
        sync(source),
        "TypeError,0,AsyncDisposableStack,0,disposeAsync,true,get disposed,0,false"
    );
}

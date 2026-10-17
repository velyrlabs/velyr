//! `Iterator.prototype[Symbol.dispose]` and `AsyncIterator.prototype[Symbol.asyncDispose]` (N32).

use velyr_engine_api::{
    Engine, Isolate, IsolateOptions, MicrotaskProgress, Source, Value,
};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn eval(iso: &mut NativeIsolate, text: &str) -> String {
    let source = Source {
        name: "iterator_dispose.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(Value::String(s)) => s.to_string_lossy(),
        Ok(Value::Bool(b)) => b.to_string(),
        Ok(Value::Number(n)) => n.to_string(),
        Ok(Value::Undefined) => "undefined".into(),
        Ok(other) => panic!("`{text}` returned {other:?}"),
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

fn run(text: &str) -> String {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    eval(&mut iso, text)
}

/// Runs `setup`, drains the microtask queue and returns `String(log)`.
fn log_after(setup: &str) -> String {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    eval(&mut iso, &format!("var log = []; {setup}; 0"));
    while iso.perform_microtask_checkpoint() == MicrotaskProgress::Progressed {}
    eval(&mut iso, "String(log)")
}

#[test]
fn dispose_calls_return_once() {
    let source = "var closed = 0;
        var it = { next() { return { done: true }; }, return() { closed++; return {}; } };
        Object.setPrototypeOf(it, Iterator.prototype);
        var result = it[Symbol.dispose]();
        [closed, result].join()";
    assert_eq!(run(source), "1,");
}

#[test]
fn dispose_without_return_is_a_noop() {
    let source = "var it = Object.create(Iterator.prototype); String(it[Symbol.dispose]())";
    assert_eq!(run(source), "undefined");
}

#[test]
fn dispose_shape() {
    let source = "var f = Iterator.prototype[Symbol.dispose];
        [f.length, f.name].join()";
    assert_eq!(run(source), "0,[Symbol.dispose]");
}

#[test]
fn dispose_works_on_builtin_iterators() {
    assert_eq!(run("var it = [1, 2][Symbol.iterator](); String(it[Symbol.dispose]())"), "undefined");
}

#[test]
fn async_dispose_shape() {
    let source = "var AIP = Object.getPrototypeOf(Object.getPrototypeOf(async function*(){}.prototype));
        var f = AIP[Symbol.asyncDispose];
        [f.length, f.name].join()";
    assert_eq!(run(source), "0,[Symbol.asyncDispose]");
}

#[test]
fn async_dispose_awaits_return() {
    let source = "var AIP = Object.getPrototypeOf(Object.getPrototypeOf(async function*(){}.prototype));
        var it = Object.create(AIP);
        it.return = function () { log.push('return'); return Promise.resolve({}); };
        var p = it[Symbol.asyncDispose]();
        log.push(p instanceof Promise);
        p.then(v => log.push('done:' + v))";
    assert_eq!(log_after(source), "return,true,done:undefined");
}

#[test]
fn async_dispose_without_return_resolves() {
    let source = "var AIP = Object.getPrototypeOf(Object.getPrototypeOf(async function*(){}.prototype));
        Object.create(AIP)[Symbol.asyncDispose]().then(v => log.push('done:' + v))";
    assert_eq!(log_after(source), "done:undefined");
}

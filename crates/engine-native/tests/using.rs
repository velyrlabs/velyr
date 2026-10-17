//! `using` and `await using` declarations (explicit resource management, L24).

use velyr_engine_api::{Engine, Isolate, IsolateOptions, MicrotaskProgress, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, text: &str) -> String {
    let source = Source {
        name: "using.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(Value::String(s)) => s.to_string_lossy(),
        Ok(Value::Number(n)) => n.to_string(),
        Ok(other) => panic!("`{text}` returned {other:?}"),
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

fn run(text: &str) -> String {
    eval(&mut isolate(), text)
}

/// Runs `setup`, drains the microtask queue and returns `String(log)`.
fn log_after(setup: &str) -> String {
    let mut iso = isolate();
    eval(&mut iso, &format!("var log = []; {setup}; ''"));
    while iso.perform_microtask_checkpoint() == MicrotaskProgress::Progressed {}
    eval(&mut iso, "String(log)")
}

#[test]
fn disposes_in_reverse_on_exit() {
    assert_eq!(
        run(
            "var log = []; function r(n) { return { [Symbol.dispose]() { log.push(n); } }; } { using a = r(1), b = r(2); log.push(0); } log.join()"
        ),
        "0,2,1"
    );
}

#[test]
fn disposes_on_return_and_throw() {
    assert_eq!(
        run(
            "var log = []; function f() { using a = { [Symbol.dispose]() { log.push('d'); } }; return 'r'; } f() + log"
        ),
        "rd"
    );
    assert_eq!(
        run(
            "var log = []; try { using a = { [Symbol.dispose]() { log.push('d'); } }; throw 't'; } catch (e) { log.push(e); } log.join()"
        ),
        "d,t"
    );
}

#[test]
fn disposes_on_break_and_continue() {
    assert_eq!(
        run(
            "var log = []; for (var i = 0; i < 3; i++) { using a = { [Symbol.dispose]() { log.push(i); } }; if (i == 0) continue; break; } log.join()"
        ),
        "0,1"
    );
}

#[test]
fn suppressed_error() {
    assert_eq!(
        run(
            "try { using a = { [Symbol.dispose]() { throw 'd'; } }; throw 'b'; } catch (e) { e.constructor.name + e.error + e.suppressed }"
        ),
        "SuppressedErrordb"
    );
}

#[test]
fn using_binding_is_const() {
    assert_eq!(
        run("try { using a = null; a = 1; 'no' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn for_of_using_disposes_each_iteration() {
    assert_eq!(
        run("var n = 0; for (using x of [{ [Symbol.dispose]() { n++; } }, null]) {} n"),
        "1"
    );
}

#[test]
fn await_using_awaits_async_dispose() {
    assert_eq!(
        log_after(
            "(async function () {
                { await using a = { async [Symbol.asyncDispose]() { log.push('d'); } }; log.push('b'); }
                log.push('after');
            })(); log.push('sync')"
        ),
        "b,d,sync,after"
    );
}

#[test]
fn for_await_of_await_using() {
    assert_eq!(
        log_after(
            "(async function () {
                for await (await using x of [{ [Symbol.asyncDispose]() { log.push('x'); } }, null]) log.push('body');
                log.push('end');
            })()"
        ),
        "body,x,body,end"
    );
}

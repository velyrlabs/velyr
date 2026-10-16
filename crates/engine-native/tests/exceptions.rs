//! Exceptions on the native engine (VELYR-106): throw/try/catch/finally completions,
//! V8-format `Error.prototype.stack`, `Error.captureStackTrace`, `Error.stackTraceLimit`
//! and the `JsException` an uncaught exception becomes.

use velyr_engine_api::*;
use velyr_engine_native::NativeEngine;

fn eval(text: &str) -> Result<Value, JsException> {
    let mut isolate = NativeEngine
        .create_isolate(IsolateOptions::default())
        .unwrap();
    isolate.eval_script(&Source {
        name: "test.js".into(),
        text: text.into(),
    })
}

fn string(text: &str) -> String {
    match eval(text) {
        Ok(Value::String(s)) => s.to_string_lossy(),
        other => panic!("expected a string from {text:?}, got {other:?}"),
    }
}

#[test]
fn finally_return_overrides_and_cancels_completions() {
    let source = "
        function overrideReturn() { try { return 'try'; } finally { return 'finally'; } }
        function cancelThrow() { try { throw 1; } finally { return 'swallowed'; } }
        function replaceThrow() {
            try { try { throw 'first'; } finally { throw 'second'; } } catch (e) { return e; }
        }
        function breakOut() {
            let log = '';
            for (const x of [1, 2]) { try { log += x; break; } finally { log += 'f'; } }
            return log;
        }
        [overrideReturn(), cancelThrow(), replaceThrow(), breakOut()] + '';";
    assert_eq!(string(source), "finally,swallowed,second,1f");
}

#[test]
fn try_statement_completion_value_ignores_finally() {
    assert_eq!(string("try { 'try' } finally { 'finally' }"), "try");
    assert_eq!(
        string("try { throw 1 } catch { 'catch' } finally { 'f' }"),
        "catch"
    );
}

#[test]
fn error_stack_lists_frames_in_v8_format() {
    let source = "function inner() { return new Error('boom'); }
function outer() { return inner(); }
outer().stack";
    assert_eq!(
        string(source),
        "Error: boom\n    at inner (test.js:1:27)\n    at outer (test.js:2:27)\n    at test.js:3:1"
    );
}

#[test]
fn engine_errors_and_constructor_frames_have_stacks() {
    let source = "function C() { notDefined; }
let stack;
try { new C(); } catch (e) { stack = e.stack; }
stack";
    assert_eq!(
        string(source),
        "ReferenceError: notDefined is not defined\n    at new C (test.js:1:16)\n    at test.js:3:7"
    );
}

#[test]
fn stack_is_a_non_enumerable_writable_own_property() {
    let source = "
        const e = new RangeError('r');
        let keys = '';
        for (const k in e) keys += k;
        e.stack = 'replaced';
        [keys === '', e.stack] + '';";
    assert_eq!(string(source), "true,replaced");
}

#[test]
fn stack_trace_limit_bounds_and_disables_frames() {
    let source = "function a() { return b(); }
function b() { return new Error('x').stack; }
Error.stackTraceLimit = 1;
const one = a();
Error.stackTraceLimit = 0;
const none = a();
Error.stackTraceLimit = 'not a number';
[one, none, typeof new Error('y').stack] + ''";
    assert_eq!(
        string(source),
        "Error: x\n    at b (test.js:2:23),Error: x,undefined"
    );
}

#[test]
fn capture_stack_trace_installs_stack_and_hides_frames_above_the_function() {
    let source = "const target = { message: 'custom' };
function a() { b(); }
function b() { Error.captureStackTrace(target, b); }
a();
const plain = {};
Error.captureStackTrace(plain);
[target.stack, plain.stack] + ''";
    assert_eq!(
        string(source),
        "Error: custom\n    at a (test.js:2:16)\n    at test.js:4:1,Error\n    at test.js:6:7"
    );
}

#[test]
fn uncaught_error_becomes_js_exception_with_message_and_stack() {
    let error = eval("function f() { throw new TypeError('bad'); }\nf();").unwrap_err();
    assert_eq!(error.message, "TypeError: bad");
    assert_eq!(
        error.stack.as_deref(),
        Some("TypeError: bad\n    at f (test.js:1:22)\n    at test.js:2:1")
    );
    assert!(error.value.is_some());
}

#[test]
fn uncaught_non_error_value_has_v8_message_and_no_stack() {
    let error = eval("throw 'oops';").unwrap_err();
    assert_eq!(error.message, "oops");
    assert_eq!(error.stack, None);
}

#[test]
fn eval_and_uri_error_constructors_exist() {
    let source = "
        const e = new EvalError('1'), u = URIError('2');
        [e + '', u + '', e instanceof Error, u instanceof URIError] + '';";
    assert_eq!(string(source), "EvalError: 1,URIError: 2,true,true");
}

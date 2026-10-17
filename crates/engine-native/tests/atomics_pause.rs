//! `Atomics.pause` (proposal-atomics-microwait) and `Atomics.wait` on the main thread.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::NativeEngine;

fn eval(code: &str) -> Value {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "atomics_pause.js".into(),
        text: code.into(),
    };
    match iso.eval_script(&source) {
        Ok(value) => value,
        Err(error) => panic!("`{code}` threw {}", error.message),
    }
}

fn text(s: &str) -> Value {
    Value::String(JsString::from(s))
}

fn throws_type_error(argument: &str) -> String {
    format!("try {{ Atomics.pause({argument}); 'no throw' }} catch (e) {{ e.constructor.name }}")
}

#[test]
fn pause_returns_undefined_and_has_length_zero() {
    assert_eq!(eval("Atomics.pause()"), Value::Undefined);
    assert_eq!(eval("Atomics.pause(5)"), Value::Undefined);
    assert_eq!(eval("Atomics.pause(-0)"), Value::Undefined);
    assert_eq!(eval("Atomics.pause.length"), Value::Number(0.0));
}

#[test]
fn pause_rejects_non_integral_arguments() {
    for argument in ["1.5", "'1'", "Infinity", "NaN", "null", "1n"] {
        assert_eq!(
            eval(&throws_type_error(argument)),
            text("TypeError"),
            "{argument}"
        );
    }
}

#[test]
fn wait_is_allowed_on_the_main_thread() {
    let code = "Atomics.wait(new Int32Array(new SharedArrayBuffer(8)), 0, 0, 0)";
    assert_eq!(eval(code), text("timed-out"));
}

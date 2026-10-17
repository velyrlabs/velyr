//! Host hooks: `Isolate::check_script`, `CallContext::eval_script` and `CallContext::request_gc`.

use velyr_engine_api::{
    CallContext, Engine, HostError, HostFunctionDef, Isolate, IsolateOptions, JsString, Source,
    Value,
};
use velyr_engine_native::NativeEngine;

fn hook(cx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Value::String(text) = &args[0] else {
        unreachable!()
    };
    cx.request_gc()?;
    cx.eval_script(&Source {
        name: "<evalScript>".into(),
        text: text.to_string_lossy(),
    })
}

fn source(text: &str) -> Source {
    Source {
        name: "t".into(),
        text: text.into(),
    }
}

#[test]
fn eval_script_from_host_function_shares_global() {
    let mut isolate = NativeEngine
        .create_isolate(IsolateOptions::default())
        .unwrap();
    let def = HostFunctionDef {
        name: "evalScript",
        arity: 1,
        callback: hook,
    };
    isolate.register_host_function(def).unwrap();
    let value = isolate
        .eval_script(&source("evalScript('var z = 41;'); z + 1"))
        .unwrap();
    assert_eq!(value, Value::Number(42.0));
}

#[test]
fn eval_script_syntax_error_is_catchable() {
    let mut isolate = NativeEngine
        .create_isolate(IsolateOptions::default())
        .unwrap();
    let def = HostFunctionDef {
        name: "evalScript",
        arity: 1,
        callback: hook,
    };
    isolate.register_host_function(def).unwrap();
    let value = isolate
        .eval_script(&source(
            "try { evalScript('let;let x;let x'); 'no' } catch (e) { e.name }",
        ))
        .unwrap();
    assert_eq!(value, Value::String(JsString::from("SyntaxError")));
}

#[test]
fn check_script_reports_early_errors_without_running() {
    let mut isolate = NativeEngine
        .create_isolate(IsolateOptions::default())
        .unwrap();
    assert!(
        isolate
            .check_script(&source("throw 1; let x; let x;"))
            .is_err()
    );
    assert!(isolate.check_script(&source("globalThis.ran = 1;")).is_ok());
    assert_eq!(
        isolate.eval_script(&source("typeof ran")).unwrap(),
        Value::String(JsString::from("undefined"))
    );
}

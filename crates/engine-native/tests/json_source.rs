//! JSON.parse source text access and JSON.rawJSON (proposal-json-parse-with-source).

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::NativeEngine;

fn eval(text: &str) -> Value {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "json_source.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(value) => value,
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

fn text(s: &str) -> Value {
    Value::String(JsString::from(s))
}

#[test]
fn reviver_receives_primitive_source_text() {
    let code = "var log; JSON.parse('[1.0,2]', function (k, v, c) { if (k === '0') log = c.source; return v; }); log";
    assert_eq!(eval(code), text("1.0"));
}

#[test]
fn object_values_get_a_context_without_source() {
    let code = "var r; JSON.parse('{}', function (k, v, c) { if (k === '') r = 'source' in c; return v; }); r";
    assert_eq!(eval(code), Value::Bool(false));
}

#[test]
fn modified_value_loses_source() {
    let code = "var r; JSON.parse('[1,2]', function (k, v, c) { if (k === '0') this[1] = 3; if (k === '1') r = 'source' in c; return v; }); r";
    assert_eq!(eval(code), Value::Bool(false));
}

#[test]
fn stringify_emits_raw_json_verbatim() {
    assert_eq!(
        eval("JSON.stringify({a: JSON.rawJSON('1e1000')})"),
        text("{\"a\":1e1000}")
    );
}

#[test]
fn is_raw_json_detects_raw_objects() {
    assert_eq!(eval("JSON.isRawJSON(JSON.rawJSON('1'))"), Value::Bool(true));
    assert_eq!(eval("JSON.isRawJSON({})"), Value::Bool(false));
}

#[test]
fn raw_json_rejects_invalid_text() {
    for bad in ["' 1'", "'1 '", "''", "'{}'", "'[]'", "'1 2'"] {
        let code =
            format!("try {{ JSON.rawJSON({bad}); 'no' }} catch (e) {{ e instanceof SyntaxError }}");
        assert_eq!(eval(&code), Value::Bool(true), "{bad}");
    }
}

#[test]
fn raw_json_object_is_frozen_with_null_prototype() {
    assert_eq!(
        eval("Object.isFrozen(JSON.rawJSON('1'))"),
        Value::Bool(true)
    );
    assert_eq!(
        eval("Object.getPrototypeOf(JSON.rawJSON('1')) === null"),
        Value::Bool(true)
    );
    assert_eq!(eval("JSON.rawJSON('\"a\"').rawJSON"), text("\"a\""));
}

#[test]
fn function_lengths() {
    assert_eq!(
        eval("JSON.rawJSON.length + JSON.isRawJSON.length"),
        Value::Number(2.0)
    );
}

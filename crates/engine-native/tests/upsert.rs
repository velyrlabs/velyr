//! `Map` and `WeakMap` `getOrInsert` / `getOrInsertComputed` (proposal-upsert).

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::NativeEngine;

fn eval(code: &str) -> Value {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "upsert.js".into(),
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

fn error_name(body: &str) -> Value {
    eval(&format!(
        "try {{ {body}; 'no throw' }} catch (e) {{ e.constructor.name }}"
    ))
}

#[test]
fn get_or_insert_canonicalizes_negative_zero() {
    let code = "const m = new Map(); m.getOrInsert(-0, 'a'); Object.is([...m.keys()][0], 0)";
    assert_eq!(eval(code), Value::Bool(true));
}

#[test]
fn get_or_insert_keeps_the_first_value() {
    let code = "const m = new Map(); m.getOrInsert(1, 'x'); m.getOrInsert(1, 'y')";
    assert_eq!(eval(code), text("x"));
}

#[test]
fn computed_calls_back_once_with_the_key_and_never_when_present() {
    let code = "const m = new Map(); let calls = 0, seen;
        const f = (k) => { calls++; seen = k; return 'v'; };
        m.getOrInsertComputed(7, f); m.getOrInsertComputed(7, f);
        `${calls}:${seen}:${m.get(7)}`";
    assert_eq!(eval(code), text("1:7:v"));
}

#[test]
fn computed_callback_overwrites_a_key_it_inserted() {
    let code = "const m = new Map();
        const r = m.getOrInsertComputed(1, () => { m.set(1, 'inner'); return 'outer'; });
        `${r}:${m.get(1)}:${m.size}`";
    assert_eq!(eval(code), text("outer:outer:1"));
}

#[test]
fn computed_rejects_a_non_callable_even_when_the_key_exists() {
    let code = "const m = new Map([[1, 2]]); m.getOrInsertComputed(1, 5)";
    assert_eq!(error_name(code), text("TypeError"));
}

#[test]
fn map_methods_reject_incompatible_receivers() {
    let call = "Map.prototype.getOrInsert.call({}, 1, 2)";
    assert_eq!(error_name(call), text("TypeError"));
    let call = "Map.prototype.getOrInsertComputed.call(new WeakMap(), 1, () => 2)";
    assert_eq!(error_name(call), text("TypeError"));
}

#[test]
fn lengths_are_two() {
    let code = "`${Map.prototype.getOrInsert.length}${Map.prototype.getOrInsertComputed.length}\
        ${WeakMap.prototype.getOrInsert.length}${WeakMap.prototype.getOrInsertComputed.length}`";
    assert_eq!(eval(code), text("2222"));
}

#[test]
fn weak_map_inserts_and_returns_existing_values() {
    let code = "const w = new WeakMap(), k = {};
        const a = w.getOrInsert(k, 'x'), b = w.getOrInsert(k, 'y');
        const c = w.getOrInsertComputed(Symbol(), () => 'z');
        `${a}${b}${c}${w.get(k)}`";
    assert_eq!(eval(code), text("xxzx"));
}

#[test]
fn weak_map_rejects_invalid_keys_before_the_callable_check() {
    assert_eq!(
        error_name("new WeakMap().getOrInsert(1, 2)"),
        text("TypeError")
    );
    let code = "new WeakMap().getOrInsertComputed(1, 5)";
    assert_eq!(error_name(code), text("TypeError"));
    let code = "let called = false; try { new WeakMap().getOrInsertComputed(1, () => { called = true; }) } catch (e) {} called";
    assert_eq!(eval(code), Value::Bool(false));
    let code = "new WeakMap().getOrInsertComputed({}, 5)";
    assert_eq!(error_name(code), text("TypeError"));
}

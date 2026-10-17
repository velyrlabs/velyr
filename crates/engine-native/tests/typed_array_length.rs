//! TypedArray.prototype methods read the internal length, not the `length` property.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::NativeEngine;

fn run(code: &str) -> String {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "typed_array_length.js".into(),
        text: format!("String((() => {{ {code} }})())"),
    };
    match iso.eval_script(&source) {
        Ok(Value::String(text)) => text.to_string_lossy(),
        Ok(other) => format!("non-string {other:?}"),
        Err(error) => format!("throw {}", error.message),
    }
}

const SHADOWED: &str = "const ta = new Int8Array([1,2,3]); \
    Object.defineProperty(ta, 'length', { value: 1 });";

#[test]
fn for_each_ignores_shadowed_length() {
    let code = format!("{SHADOWED} let n = 0; ta.forEach(() => n++); return n;");
    assert_eq!(run(&code), "3");
}

#[test]
fn shrunk_buffer_still_visits_every_index() {
    let code = "const rab = new ArrayBuffer(4, { maxByteLength: 8 }); \
        const ta = new Uint8Array(rab); const seen = []; \
        ta.forEach((v, i) => { if (i === 0) rab.resize(2); seen.push(v); }); \
        return seen.join();";
    assert_eq!(run(code), "0,0,,");
}

#[test]
fn array_generic_methods_still_read_length() {
    let code = "let reads = 0; const o = { 0: 'a', get length() { reads++; return 1; } }; \
        Array.prototype.forEach.call(o, () => {}); return reads;";
    assert_eq!(run(code), "1");
}

#[test]
fn search_and_join_use_internal_length() {
    let code = format!(
        "{SHADOWED} return [ta.includes(3), ta.indexOf(3), ta.lastIndexOf(3), ta.join(), \
         ta.at(-1), ta.reduce((a, b) => a + b), ta.findLast(() => true)].join('|');"
    );
    assert_eq!(run(&code), "true|2|2|1,2,3|3|6|3");
}

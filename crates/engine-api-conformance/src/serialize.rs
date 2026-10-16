//! Structured-clone cases; engines without `SERIALIZE` skip them.

use velyr_engine_api::{Engine, EngineFeatures, Isolate, IsolateOptions, Source, Value};

use crate::Case;

type CaseResult = Result<(), String>;

pub(crate) fn all<E: Engine>() -> Vec<Case<E>> {
    let gated = |name, run| Case {
        name,
        requires: Some(("SERIALIZE", EngineFeatures::SERIALIZE)),
        run,
    };
    vec![
        gated("serialize_deep_clone", deep_clone::<E>),
        gated("serialize_function_throws", function_throws::<E>),
        gated("serialize_transfer_detaches", transfer_detaches::<E>),
        gated(
            "serialize_transfer_is_single_use",
            transfer_is_single_use::<E>,
        ),
    ]
}

fn isolate<E: Engine>(engine: &E) -> Result<E::Isolate, String> {
    engine
        .create_isolate(IsolateOptions::default())
        .map_err(|e| format!("create_isolate: {e}"))
}

fn eval(iso: &mut impl Isolate, text: &str) -> Result<Value, String> {
    let source = Source {
        name: "conformance-serialize.js".into(),
        text: text.into(),
    };
    iso.eval_script(&source)
        .map_err(|e| format!("eval `{text}`: {}", e.message))
}

fn clone_of(iso: &mut impl Isolate, original: &Value) -> Result<Value, String> {
    let serialized = iso
        .serialize(original, &[])
        .map_err(|e| format!("serialize: {}", e.message))?;
    iso.deserialize(&serialized)
        .map_err(|e| format!("deserialize: {}", e.message))
}

fn deep_clone<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    let original = eval(
        &mut iso,
        "const o = { map: new Map([[1, { a: 'x' }]]), set: new Set([1, 'two']), \
         date: new Date(1700000000000), re: /a+b/gi, bytes: new Uint8Array([1, 2, 3]), \
         floats: new Float64Array([1.5, -2]), list: [1, [2, 3]], big: 10n, text: 'h\\u{1F600}' }; \
         o.self = o; o",
    )?;
    let clone = clone_of(&mut iso, &original)?;
    let same = eval(
        &mut iso,
        "(o, c) => c !== o && c.self === c && c.map !== o.map && c.map.get(1).a === 'x' \
         && c.set.has('two') && c.date instanceof Date && c.date.getTime() === 1700000000000 \
         && c.re instanceof RegExp && String(c.re) === '/a+b/gi' \
         && c.bytes instanceof Uint8Array && c.bytes.join() === '1,2,3' && c.bytes.buffer !== o.bytes.buffer \
         && c.floats.join() === '1.5,-2' && c.list[1][1] === 3 && c.big === 10n && c.text === o.text",
    )?;
    let verdict = iso
        .call(&same, &Value::Undefined, &[original, clone])
        .map_err(|e| format!("compare: {}", e.message))?;
    if verdict == Value::Bool(true) {
        Ok(())
    } else {
        Err(format!("clone differs from original: {verdict:?}"))
    }
}

fn function_throws<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    let function = eval(&mut iso, "(function f() {})")?;
    match iso.serialize(&function, &[]) {
        Err(e) if e.message.starts_with("DataCloneError") => Ok(()),
        Err(e) => Err(format!("expected DataCloneError, got `{}`", e.message)),
        Ok(_) => Err("serializing a function succeeded".into()),
    }
}

fn transfer_detaches<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    let original = eval(
        &mut iso,
        "const b = new ArrayBuffer(8); new Uint8Array(b)[0] = 7; b",
    )?;
    let serialized = iso
        .serialize(&original, std::slice::from_ref(&original))
        .map_err(|e| format!("serialize: {}", e.message))?;
    let clone = iso
        .deserialize(&serialized)
        .map_err(|e| format!("deserialize: {}", e.message))?;
    let check = eval(
        &mut iso,
        "(o, c) => o.byteLength === 0 && c.byteLength === 8 && new Uint8Array(c)[0] === 7",
    )?;
    let verdict = iso
        .call(&check, &Value::Undefined, &[original, clone])
        .map_err(|e| format!("check: {}", e.message))?;
    if verdict == Value::Bool(true) {
        Ok(())
    } else {
        Err(format!("transfer did not detach the original: {verdict:?}"))
    }
}

fn transfer_is_single_use<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    let original = eval(&mut iso, "new ArrayBuffer(8)")?;
    let serialized = iso
        .serialize(&original, std::slice::from_ref(&original))
        .map_err(|e| format!("serialize: {}", e.message))?;
    iso.deserialize(&serialized)
        .map_err(|e| format!("first deserialize: {}", e.message))?;
    match iso.deserialize(&serialized) {
        Err(_) => Ok(()),
        Ok(_) => Err("a transferred buffer was deserialized twice".into()),
    }
}

use velyr_engine_api::{Engine, EngineFeatures, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_v8::{V8Engine, V8Isolate};

fn isolate() -> V8Isolate {
    V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut V8Isolate, text: &str) -> Value {
    let source = Source {
        name: "test.js".into(),
        text: text.into(),
    };
    iso.eval_script(&source).expect("eval")
}

#[test]
fn engine_declares_serialize() {
    assert!(
        V8Engine::new()
            .features()
            .contains(EngineFeatures::SERIALIZE)
    );
}

#[test]
fn function_throws_data_clone_error_with_name_and_code() {
    let mut iso = isolate();
    let function = eval(&mut iso, "(() => 1)");
    let err = iso.serialize(&function, &[]).expect_err("must not clone");
    let thrown = err.value.expect("thrown value");
    let probe = eval(
        &mut iso,
        "(e) => e.name + ':' + e.code + ':' + Object.keys(e).length",
    );
    let out = iso
        .call(&probe, &Value::Undefined, &[thrown])
        .expect("probe");
    assert_eq!(out, Value::String(JsString::from("DataCloneError:25:0")));
}

#[test]
fn non_buffer_in_transfer_list_is_rejected() {
    let mut iso = isolate();
    let object = eval(&mut iso, "({})");
    let err = iso
        .serialize(&object, std::slice::from_ref(&object))
        .expect_err("object is not transferable");
    assert!(err.message.starts_with("DataCloneError"), "{}", err.message);
}

#[test]
fn already_detached_buffer_cannot_be_transferred_again() {
    let mut iso = isolate();
    let buffer = eval(&mut iso, "new ArrayBuffer(4)");
    let transfer = std::slice::from_ref(&buffer);
    iso.serialize(&buffer, transfer).expect("first transfer");
    let err = iso
        .serialize(&buffer, transfer)
        .expect_err("second transfer");
    assert!(err.message.starts_with("DataCloneError"), "{}", err.message);
}

#[test]
fn failed_serialize_leaves_transferred_buffer_attached() {
    let mut iso = isolate();
    let bag = eval(
        &mut iso,
        "globalThis.b = new ArrayBuffer(4); ({ b, f() {} })",
    );
    let buffer = eval(&mut iso, "b");
    iso.serialize(&bag, std::slice::from_ref(&buffer))
        .expect_err("function inside");
    assert_eq!(eval(&mut iso, "b.byteLength"), Value::Number(4.0));
}

#[test]
fn serialized_value_moves_to_another_isolate_on_another_thread() {
    let mut source = isolate();
    let original = eval(
        &mut source,
        "globalThis.b = new Uint8Array([9, 8, 7]).buffer; ({ b, m: new Map([['k', 1]]) })",
    );
    let buffer = eval(&mut source, "b");
    let serialized = source
        .serialize(&original, std::slice::from_ref(&buffer))
        .expect("serialize");
    assert_eq!(eval(&mut source, "b.byteLength"), Value::Number(0.0));

    let out = std::thread::spawn(move || {
        let mut target = isolate();
        let clone = target.deserialize(&serialized).expect("deserialize");
        let probe = eval(
            &mut target,
            "(c) => new Uint8Array(c.b).join() + '/' + c.m.get('k')",
        );
        target
            .call(&probe, &Value::Undefined, &[clone])
            .expect("probe")
    })
    .join()
    .expect("thread");
    assert_eq!(out, Value::String(JsString::from("9,8,7/1")));
}

#[test]
fn primitives_round_trip() {
    let mut iso = isolate();
    for value in [
        Value::Undefined,
        Value::Null,
        Value::Bool(true),
        Value::Number(-0.5),
        Value::String(JsString::from_utf16(vec![0x61, 0xD800])),
    ] {
        let serialized = iso.serialize(&value, &[]).expect("serialize");
        assert_eq!(iso.deserialize(&serialized).expect("deserialize"), value);
    }
}

#[test]
fn corrupt_bytes_fail_to_deserialize() {
    let mut iso = isolate();
    let mut serialized = iso.serialize(&Value::Number(1.0), &[]).expect("serialize");
    serialized.bytes = vec![0xFF, 0xFF, 0xFF];
    assert!(iso.deserialize(&serialized).is_err());
}

#[test]
fn transferred_buffer_is_consumed_by_first_deserialize() {
    let mut source = isolate();
    let buffer = eval(&mut source, "new Uint8Array([1, 2, 3]).buffer");
    let serialized = source
        .serialize(&buffer, std::slice::from_ref(&buffer))
        .expect("serialize");

    let mut first = isolate();
    first.deserialize(&serialized).expect("first deserialize");

    let mut second = isolate();
    let error = second
        .deserialize(&serialized)
        .expect_err("second deserialize must fail");
    assert!(error.message.contains("already"), "{}", error.message);
}

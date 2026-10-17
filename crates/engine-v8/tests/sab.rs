use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
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
fn sab_structured_clone_is_rejected_today() {
    let mut iso = isolate();
    let sab = eval(&mut iso, "new SharedArrayBuffer(8)");
    let err = iso
        .serialize(&sab, &[])
        .expect_err("SAB clone is unsupported");
    // V8 rejects SAB before the delegate runs: a plain Error, not a DataCloneError.
    assert!(
        err.message.contains("could not be cloned"),
        "{}",
        err.message
    );
}

#[test]
#[ignore = "RT-07"]
fn sab_roundtrip_shares_memory() {
    let mut a = isolate();
    let sab = eval(&mut a, "globalThis.sab = new SharedArrayBuffer(8)");
    let serialized = a.serialize(&sab, &[]).expect("serialize SAB");
    std::thread::spawn(move || {
        let mut b = isolate();
        let in_b = b.deserialize(&serialized).expect("deserialize SAB");
        let write = eval(&mut b, "(s) => { new Uint8Array(s)[0] = 42; }");
        b.call(&write, &Value::Undefined, &[in_b])
            .expect("write in B");
    })
    .join()
    .expect("isolate B thread");
    assert_eq!(eval(&mut a, "new Uint8Array(sab)[0]"), Value::Number(42.0));
}

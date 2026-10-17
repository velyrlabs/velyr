//! `create_array_buffer_owned` hands the `Vec` allocation to V8 instead of copying it.

use velyr_engine_api::{
    CallContext, Engine, HostError, HostFunctionDef, Isolate, IsolateOptions, JsString, Source,
    Value,
};
use velyr_engine_v8::V8Engine;

const FOUR_MIB: usize = 4 << 20;

/// Returns the address of the bytes behind its `Uint8Array` argument.
fn data_address(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let mut address = 0;
    let borrowed = ctx.with_bytes(&args[0], &mut |bytes| address = bytes.as_ptr() as usize);
    assert!(borrowed, "argument is not a borrowable view");
    Ok(Value::Number(address as f64))
}

#[test]
fn owned_bytes_back_the_array_buffer_without_a_copy() {
    let mut isolate = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let def = HostFunctionDef {
        name: "dataAddress",
        arity: 1,
        callback: data_address,
    };
    isolate.register_host_function(def).expect("register");
    let bytes = vec![7u8; FOUR_MIB];
    let vec_address = bytes.as_ptr() as usize;

    let buffer = isolate.create_array_buffer_owned(bytes);
    let source = Source {
        name: "probe.js".into(),
        text:
            "(b) => [dataAddress(new Uint8Array(b)), b.byteLength, new Uint8Array(b).at(-1)].join()"
                .into(),
    };
    let probe = isolate.eval_script(&source).expect("probe");
    let seen = isolate
        .call(&probe, &Value::Undefined, &[buffer])
        .expect("call");
    let expected = format!("{vec_address},{FOUR_MIB},7");
    assert_eq!(seen, Value::String(JsString::from(expected.as_str())));
}

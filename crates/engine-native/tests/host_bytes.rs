//! Binary data across the host boundary on engine-native (VELYR-139): `read_bytes` sees
//! exactly the window of a buffer or view, `create_array_buffer` hands bytes back.

use velyr_engine_api::*;
use velyr_engine_native::NativeEngine;

/// `echo(value)`: the bytes `value` covers, copied into a new ArrayBuffer.
fn echo(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    match ctx.read_bytes(&args[0]) {
        Some(bytes) => Ok(ctx.create_array_buffer(&bytes)),
        None => Err(HostError::TypeError("echo expects binary data".into())),
    }
}

fn run(text: &str) -> Result<Value, JsException> {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .unwrap();
    iso.register_host_function(HostFunctionDef {
        name: "echo",
        arity: 1,
        callback: echo,
    })
    .unwrap();
    iso.eval_script(&Source {
        name: "host_bytes.js".into(),
        text: text.into(),
    })
}

fn text(s: &str) -> Value {
    Value::String(JsString::from(s))
}

#[test]
fn every_byte_value_round_trips() {
    let script = "const all = Uint8Array.from({ length: 256 }, (_, i) => i);
        const back = new Uint8Array(echo(all));
        back.length === 256 && back.every((byte, i) => byte === i) ? 'ok' : String(back)";
    assert_eq!(run(script).unwrap(), text("ok"));
}

#[test]
fn views_yield_only_their_window() {
    let script = "const bytes = Uint8Array.from([0, 1, 2, 3, 4, 5, 6, 7]);
        const typed = new Uint8Array(echo(new Uint16Array(bytes.buffer, 2, 2)));
        const view = new Uint8Array(echo(new DataView(bytes.buffer, 5, 2)));
        const whole = new Uint8Array(echo(bytes.buffer));
        [typed, view, whole].map(String).join('|')";
    assert_eq!(run(script).unwrap(), text("2,3,4,5|5,6|0,1,2,3,4,5,6,7"));
}

#[test]
fn detached_buffers_and_plain_values_are_rejected() {
    let detached = run("const buffer = new ArrayBuffer(4); buffer.transfer(); echo(buffer)");
    assert!(detached.unwrap_err().message.contains("binary data"));
    let view = run(
        "const buffer = new ArrayBuffer(4); const u8 = new Uint8Array(buffer); buffer.transfer(); echo(u8)",
    );
    assert!(view.unwrap_err().message.contains("binary data"));
    let plain = run("echo('not bytes')");
    assert!(plain.unwrap_err().message.contains("binary data"));
}

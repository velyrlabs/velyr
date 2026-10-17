//! `Uint8Array` base64 and hex conversions (proposal-arraybuffer-base64).

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::NativeEngine;

fn eval(code: &str) -> Value {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "uint8_base64.js".into(),
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

fn check(code: &str, expected: &str) {
    assert_eq!(eval(code), text(expected), "{code}");
}

fn error_name(body: &str) -> Value {
    eval(&format!(
        "try {{ {body}; 'no throw' }} catch (e) {{ e.constructor.name }}"
    ))
}

#[test]
fn from_base64_decodes_padded_and_loose_input() {
    check(
        "Uint8Array.fromBase64('SGVsbG8=').join()",
        "72,101,108,108,111",
    );
    check(
        "Uint8Array.fromBase64('SGVsbG8').join()",
        "72,101,108,108,111",
    );
    check(
        "Uint8Array.fromBase64(' SGVs\\tbG\\n8= ').join()",
        "72,101,108,108,111",
    );
    check("Uint8Array.fromBase64('').length + ''", "0");
}

#[test]
fn from_base64_last_chunk_handling() {
    let strict = "{lastChunkHandling:'strict'}";
    assert_eq!(
        error_name(&format!("Uint8Array.fromBase64('SGVsbG8', {strict})")),
        text("SyntaxError")
    );
    assert_eq!(
        error_name(&format!("Uint8Array.fromBase64('SGVsbG9=', {strict})")),
        text("SyntaxError")
    );
    check(
        "Uint8Array.fromBase64('SGVsbG8', {lastChunkHandling:'stop-before-partial'}).join()",
        "72,101,108",
    );
    assert_eq!(
        error_name("Uint8Array.fromBase64('SGVsb')"),
        text("SyntaxError")
    );
    assert_eq!(
        error_name("Uint8Array.fromBase64('SGVsbG8===')"),
        text("SyntaxError")
    );
    assert_eq!(
        error_name("Uint8Array.fromBase64('S')"),
        text("SyntaxError")
    );
}

#[test]
fn from_base64_alphabets() {
    check(
        "Uint8Array.fromBase64('-_8=', {alphabet:'base64url'}).join()",
        "251,255",
    );
    assert_eq!(
        error_name("Uint8Array.fromBase64('-_8=')"),
        text("SyntaxError")
    );
    assert_eq!(
        error_name("Uint8Array.fromBase64('+/8=', {alphabet:'base64url'})"),
        text("SyntaxError")
    );
    assert_eq!(
        error_name("Uint8Array.fromBase64('AA==', {alphabet:'other'})"),
        text("TypeError")
    );
    assert_eq!(error_name("Uint8Array.fromBase64(1)"), text("TypeError"));
    assert_eq!(
        error_name("Uint8Array.fromBase64('AA==', 1)"),
        text("TypeError")
    );
}

#[test]
fn to_base64_encodes_with_options() {
    check("new Uint8Array([72,101]).toBase64()", "SGU=");
    check(
        "new Uint8Array([72,101]).toBase64({omitPadding:true})",
        "SGU",
    );
    check(
        "new Uint8Array([251,255]).toBase64({alphabet:'base64url'})",
        "-_8=",
    );
    check("new Uint8Array([251,255]).toBase64()", "+/8=");
    check("new Uint8Array(0).toBase64()", "");
}

#[test]
fn hex_round_trips_and_rejects_bad_input() {
    check("Uint8Array.fromHex('48656c6c6f').toHex()", "48656c6c6f");
    check("Uint8Array.fromHex('FF0a').join()", "255,10");
    assert_eq!(error_name("Uint8Array.fromHex('abc')"), text("SyntaxError"));
    assert_eq!(error_name("Uint8Array.fromHex('zz')"), text("SyntaxError"));
    assert_eq!(error_name("Uint8Array.fromHex(1)"), text("TypeError"));
}

#[test]
fn set_from_base64_reports_read_and_written() {
    check(
        "const r = new Uint8Array(3).setFromBase64('SGVsbG8='); `${r.read}:${r.written}`",
        "4:3",
    );
    check(
        "const a = new Uint8Array(8); const r = a.setFromBase64('SGVsbG8='); `${r.read}:${r.written}:${a.join()}`",
        "8:5:72,101,108,108,111,0,0,0",
    );
    check(
        "const a = new Uint8Array(4); const r = a.setFromBase64('SGVsbG8='); `${r.read}:${r.written}`",
        "4:3",
    );
}

#[test]
fn set_from_base64_writes_before_throwing() {
    let code = "const a = new Uint8Array(8);
        try { a.setFromBase64('SGVs!') } catch (e) { e.constructor.name + ':' + a.join() }";
    check(code, "SyntaxError:72,101,108,0,0,0,0,0");
}

#[test]
fn set_from_hex_reports_read_and_written() {
    check(
        "const a = new Uint8Array(2); const r = a.setFromHex('48656c'); `${r.read}:${r.written}:${a.join()}`",
        "4:2:72,101",
    );
    let code = "const a = new Uint8Array(4);
        try { a.setFromHex('4865zz') } catch (e) { e.constructor.name + ':' + a.join() }";
    check(code, "SyntaxError:72,101,0,0");
}

#[test]
fn methods_require_a_uint8_array() {
    for call in [
        "Uint8Array.prototype.toBase64.call(new Int8Array(1))",
        "Uint8Array.prototype.toHex.call(new Uint8ClampedArray(1))",
        "Uint8Array.prototype.setFromHex.call([], '00')",
        "Uint8Array.prototype.setFromBase64.call({}, 'AA==')",
    ] {
        assert_eq!(error_name(call), text("TypeError"), "{call}");
    }
}

#[test]
fn set_from_rejects_a_detached_buffer() {
    let code = "const a = new Uint8Array(2); a.buffer.transfer(); a.setFromHex('00')";
    assert_eq!(error_name(code), text("TypeError"));
}

#[test]
fn installed_only_on_uint8_array_with_the_spec_lengths() {
    check(
        "[Uint8Array.fromBase64.length, Uint8Array.fromHex.length, Uint8Array.prototype.toBase64.length,
          Uint8Array.prototype.toHex.length, Uint8Array.prototype.setFromBase64.length,
          Uint8Array.prototype.setFromHex.length].join()",
        "1,1,0,0,1,1",
    );
    check(
        "typeof Int8Array.fromBase64 + typeof Int8Array.prototype.toHex",
        "undefinedundefined",
    );
}

#[test]
fn options_are_read_in_spec_order() {
    let code = "const log = [];
        const opts = new Proxy({}, { get(_, k) { log.push(k); return undefined; } });
        new Uint8Array(1).toBase64(opts); Uint8Array.fromBase64('', opts);
        log.join()";
    check(code, "alphabet,omitPadding,alphabet,lastChunkHandling");
}

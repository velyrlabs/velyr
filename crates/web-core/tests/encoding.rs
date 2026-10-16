//! Runs the encoding bootstrap on V8 under the real event loop.
use std::path::PathBuf;
use std::sync::Mutex;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_web_core::{encoding_host_functions, install_encoding};

// run_file executes on its own thread, so the probe records into a process-wide log.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());

const PRELUDE: &str = r#"
const hex = (bytes) => Array.from(bytes, (b) => b.toString(16).toUpperCase().padStart(2, "0")).join(" ");
const caught = (fn) => { try { fn(); return "no error"; } catch (e) { return e.name + "/" + e.constructor.name; } };
"#;

fn record(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("record expects a string".into()));
    };
    LOG.lock().unwrap().push(text.to_string_lossy());
    Ok(Value::Undefined)
}

fn run(script: &str) -> Result<Vec<String>, RunError> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let script = format!("{PRELUDE}{script}");
    let path: PathBuf = std::env::temp_dir().join(format!(
        "velyr-web-core-encoding-{}-{}.js",
        std::process::id(),
        script.len()
    ));
    std::fs::write(&path, script).unwrap();
    let mut host_functions = encoding_host_functions();
    host_functions.push(HostFunctionDef {
        name: "record",
        arity: 1,
        callback: record,
    });
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        ..RuntimeConfig::default()
    };
    let result = run_file(
        V8Engine::new(),
        config,
        host_functions,
        "",
        |isolate, _, _| install_encoding(isolate).expect("encoding bootstrap"),
        path.clone(),
    );
    std::fs::remove_file(&path).ok();
    result.map(|()| LOG.lock().unwrap().clone())
}

fn run_ok(script: &str) -> Vec<String> {
    run(script).expect("script runs")
}

#[test]
fn lone_surrogate_encodes_as_replacement_character() {
    let log = run_ok(
        r#"record(hex(new TextEncoder().encode("\uD800")));
           record(hex(new TextEncoder().encode("a\uDC00b")));"#,
    );
    assert_eq!(log, ["EF BF BD", "61 EF BF BD 62"]);
}

#[test]
fn encodes_multibyte_and_reports_encoding() {
    let log = run_ok(
        r#"const enc = new TextEncoder();
           record(enc.encoding);
           record(hex(enc.encode("€😀")));
           record(String(enc.encode().length));
           record(String(enc.encode("") instanceof Uint8Array));"#,
    );
    assert_eq!(log, ["utf-8", "E2 82 AC F0 9F 98 80", "0", "true"]);
}

#[test]
fn fatal_decoder_throws_type_error_on_invalid_bytes() {
    let log = run_ok(
        r#"const d = new TextDecoder("utf-8", { fatal: true });
           record(String(d.fatal));
           record(caught(() => d.decode(new Uint8Array([0xff, 0x41]))));
           record(d.decode(new Uint8Array([0x41])));"#,
    );
    assert_eq!(log, ["true", "TypeError/TypeError", "A"]);
}

#[test]
fn lenient_decoder_replaces_invalid_bytes() {
    let log = run_ok(
        r#"record(new TextDecoder().decode(new Uint8Array([0x41, 0xff, 0x42, 0xe2, 0x82])));"#,
    );
    assert_eq!(log, ["A\u{FFFD}B\u{FFFD}"]);
}

#[test]
fn stream_reassembles_codepoints_split_across_chunks() {
    let log = run_ok(
        r#"const d = new TextDecoder();
           const euro = [0xe2, 0x82, 0xac], emoji = [0xf0, 0x9f, 0x98, 0x80];
           let out = "";
           for (const b of [...euro, ...emoji]) out += d.decode(new Uint8Array([b]), { stream: true });
           out += d.decode();
           record(out);"#,
    );
    assert_eq!(log, ["\u{20AC}\u{1F600}"]);
}

#[test]
fn stream_flush_of_truncated_sequence_follows_fatal_flag() {
    let log = run_ok(
        r#"const lenient = new TextDecoder();
           lenient.decode(new Uint8Array([0xe2, 0x82]), { stream: true });
           record(lenient.decode());
           const strict = new TextDecoder("utf-8", { fatal: true });
           strict.decode(new Uint8Array([0xe2, 0x82]), { stream: true });
           record(caught(() => strict.decode()));
           record(strict.decode(new Uint8Array([0x41])));"#,
    );
    assert_eq!(log, ["\u{FFFD}", "TypeError/TypeError", "A"]);
}

#[test]
fn bom_is_stripped_unless_ignore_bom() {
    let log = run_ok(
        r#"const bytes = new Uint8Array([0xef, 0xbb, 0xbf, 0x41]);
           record(new TextDecoder().decode(bytes));
           const keep = new TextDecoder("utf-8", { ignoreBOM: true });
           record(String(keep.ignoreBOM));
           record(String(keep.decode(bytes).length));
           const d = new TextDecoder();
           d.decode(new Uint8Array([0xef, 0xbb]), { stream: true });
           record(d.decode(new Uint8Array([0xbf, 0x42])));"#,
    );
    assert_eq!(log, ["A", "true", "2", "B"]);
}

#[test]
fn decodes_utf16le_and_latin1_labels() {
    let log = run_ok(
        r#"const u16 = new TextDecoder("UTF-16LE");
           record(u16.encoding);
           record(u16.decode(new Uint8Array([0x41, 0x00, 0xac, 0x20])));
           const split = new TextDecoder("utf-16le");
           record(split.decode(new Uint8Array([0x41]), { stream: true }) + split.decode(new Uint8Array([0x00]), { stream: true }));
           const latin = new TextDecoder("latin1");
           record(latin.encoding);
           record(latin.decode(new Uint8Array([0x63, 0x61, 0x66, 0xe9, 0x80])));"#,
    );
    assert_eq!(
        log,
        [
            "utf-16le",
            "A\u{20AC}",
            "A",
            "windows-1252",
            "caf\u{E9}\u{20AC}"
        ]
    );
}

#[test]
fn unknown_label_throws_range_error() {
    let log = run_ok(
        r#"record(caught(() => new TextDecoder("nope")));
           record(caught(() => new TextDecoder("replacement")));
           record(new TextDecoder(" UTF8 ").encoding);"#,
    );
    assert_eq!(
        log,
        ["RangeError/RangeError", "RangeError/RangeError", "utf-8"]
    );
}

#[test]
fn decode_accepts_buffer_sources_with_offsets() {
    let log = run_ok(
        r#"const d = new TextDecoder();
           const buf = new Uint8Array([0x58, 0x68, 0x69, 0x58]).buffer;
           record(d.decode(buf));
           record(d.decode(new Uint8Array(buf, 1, 2)));
           record(d.decode(new DataView(buf, 1, 2)));
           record(caught(() => d.decode("text")));"#,
    );
    assert_eq!(log, ["XhiX", "hi", "hi", "TypeError/TypeError"]);
}

#[test]
fn encode_into_never_splits_a_character() {
    let log = run_ok(
        r#"const enc = new TextEncoder();
           const dest = new Uint8Array(5);
           const r = enc.encodeInto("a€😀", dest);
           record(r.read + "/" + r.written + "/" + hex(dest));
           const lone = enc.encodeInto("\uD800x", new Uint8Array(8));
           record(lone.read + "/" + lone.written);"#,
    );
    assert_eq!(log, ["2/4/61 E2 82 AC 00", "2/4"]);
}

#[test]
fn atob_and_btoa_round_trip() {
    let log = run_ok(
        r#"record(btoa("hello"));
           record(btoa(""));
           record(atob("aGVsbG8="));
           record(atob(" aGVs\nbG8 "));
           record(atob("aGVsbG8"));
           record(btoa(atob("/+8=")));
           record(String(atob("YQ==").charCodeAt(0)));"#,
    );
    assert_eq!(
        log,
        ["aGVsbG8=", "", "hello", "hello", "hello", "/+8=", "97"]
    );
}

#[test]
fn atob_and_btoa_reject_bad_input_with_invalid_character_error() {
    let log = run_ok(
        r#"const name = (fn) => { try { fn(); } catch (e) { return e.name; } };
           record(name(() => btoa("Ā")));
           record(name(() => atob("a")));
           record(name(() => atob("a$b=")));
           record(name(() => atob("YQ=a")));
           record(caught(() => atob()));"#,
    );
    assert_eq!(
        log,
        [
            "InvalidCharacterError",
            "InvalidCharacterError",
            "InvalidCharacterError",
            "InvalidCharacterError",
            "TypeError/TypeError"
        ]
    );
}

#[test]
fn private_ops_are_not_visible_to_user_code() {
    let log = run_ok(
        r#"record(Object.getOwnPropertyNames(globalThis).filter((n) => n.startsWith("op_")).join(","));
           record(String(Object.keys(globalThis).includes("TextEncoder")));"#,
    );
    assert_eq!(log, ["", "false"]);
}

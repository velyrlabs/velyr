//! `Blob`, `File` and object URLs on V8 under the real event loop.
use std::path::PathBuf;
use std::sync::Mutex;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_web_core::{encoding_host_functions, install_encoding, install_url, url_host_functions};

// run_file executes on its own thread, so the probe records into a process-wide log.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());

fn record(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("record expects a string".into()));
    };
    LOG.lock().unwrap().push(text.to_string_lossy());
    Ok(Value::Undefined)
}

fn run(body: &str) -> Result<Vec<String>, RunError> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let script = format!(
        "(async () => {{ {body} }})().catch((e) => record('FAIL ' + e.name + ' ' + e.message));"
    );
    let path: PathBuf = std::env::temp_dir().join(format!(
        "velyr-web-core-blob-{}-{}.js",
        std::process::id(),
        script.len()
    ));
    std::fs::write(&path, script).unwrap();
    let mut host_functions = url_host_functions();
    host_functions.extend(encoding_host_functions());
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
        |isolate, _, _| {
            install_url(isolate).expect("url bootstrap");
            install_encoding(isolate).expect("encoding bootstrap");
        },
        path.clone(),
    );
    std::fs::remove_file(&path).ok();
    result.map(|()| LOG.lock().unwrap().clone())
}

fn run_ok(body: &str) -> Vec<String> {
    run(body).expect("script runs")
}

#[test]
fn slice_returns_the_substring_without_copying_the_parent() {
    let log = run_ok(
        r#"const blob = new Blob(["hello ", "world"], { type: "Text/Plain" });
           record(blob.size + " " + blob.type);
           record(await blob.slice(2, 4).text());
           record(await blob.slice(-5).text());
           record(await blob.slice(2, 4).slice(1).text());
           const typed = blob.slice(0, 5, "X/Y");
           record(typed.type + " " + typed.size + " " + (typed instanceof Blob));
           record(String(blob.slice(9, 3).size));"#,
    );
    assert_eq!(
        log,
        ["11 text/plain", "ll", "world", "l", "x/y 5 true", "0"]
    );
}

#[test]
fn parts_can_be_strings_buffers_views_and_blobs() {
    let log = run_ok(
        r#"const inner = new Blob(["é"]);
           const blob = new Blob([new Uint8Array([104, 105]), new Uint8Array([1, 2, 3, 4]).buffer, inner, 7]);
           record(String(blob.size));
           record(Array.from(await blob.bytes()).join(","));
           record(String((await blob.arrayBuffer()).byteLength));
           record(await new Blob([]).text() === "" ? "empty" : "not empty");"#,
    );
    assert_eq!(log, ["9", "104,105,1,2,3,4,195,169,55", "9", "empty"]);
}

#[test]
fn invalid_construction_throws_type_error() {
    let log = run_ok(
        r#"for (const bad of [() => new Blob("abc"), () => new Blob(5), () => new Blob([], 5)]) {
             try { bad(); record("no error"); } catch (e) { record(e.name); }
           }"#,
    );
    assert_eq!(log, ["TypeError", "TypeError", "TypeError"]);
}

#[test]
fn file_extends_blob_with_name_and_last_modified() {
    let log = run_ok(
        r#"const file = new File(["abc"], "a.txt", { type: "text/plain", lastModified: 5 });
           record([file.name, file.lastModified, file.size, file.type, file instanceof Blob].join("|"));
           record(Object.prototype.toString.call(file));
           record(await file.slice(1).text());
           record(String(new File([], "x").lastModified > 0));
           try { new File([]); } catch (e) { record(e.name); }"#,
    );
    assert_eq!(
        log,
        [
            "a.txt|5|3|text/plain|true",
            "[object File]",
            "bc",
            "true",
            "TypeError"
        ]
    );
}

#[test]
fn object_urls_resolve_until_revoked() {
    let log = run_ok(
        r#"const blob = new Blob(["x"]);
           const url = URL.createObjectURL(blob);
           record(String(url.startsWith("blob:")) + " " + new URL(url).protocol);
           const internal = globalThis[Symbol.for("velyr.internal.blob")];
           record(String(internal.objectURL(url) === blob));
           URL.revokeObjectURL(url);
           record(String(internal.objectURL(url)));
           try { URL.createObjectURL({}); } catch (e) { record(e.name); }"#,
    );
    assert_eq!(log, ["true blob:", "true", "undefined", "TypeError"]);
}

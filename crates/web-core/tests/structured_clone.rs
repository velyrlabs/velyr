//! Runs the structuredClone bootstrap on V8 under the real event loop.
use std::path::PathBuf;
use std::sync::Mutex;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_web_core::{install_structured_clone, structured_clone_host_functions};

// run_file executes on its own thread, so the probe records into a process-wide log.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());

const PRELUDE: &str = r#"
const caught = (fn) => { try { fn(); return "no error"; } catch (e) { return e.name + "/" + e.code + "/" + (e instanceof Error); } };
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
        "velyr-web-core-structured-clone-{}-{}.js",
        std::process::id(),
        script.len()
    ));
    std::fs::write(&path, script).unwrap();
    let mut host_functions = structured_clone_host_functions();
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
        |isolate, _, _| install_structured_clone(isolate).expect("structuredClone bootstrap"),
        path.clone(),
    );
    std::fs::remove_file(&path).ok();
    result.map(|()| LOG.lock().unwrap().clone())
}

fn run_ok(script: &str) -> Vec<String> {
    run(script).expect("script runs")
}

#[test]
fn clones_collections_dates_regexps_cycles_and_typed_arrays() {
    let log = run_ok(
        r#"const o = { map: new Map([[1, { a: 1 }]]), set: new Set([1, 2]), date: new Date(5),
                       re: /x+/gi, bytes: new Uint8Array([1, 2, 3]), list: [1, [2]] };
           o.self = o;
           const c = structuredClone(o);
           record(String(c !== o && c.self === c && c.map !== o.map));
           record(String(c.map.get(1).a) + String(c.set.has(2)) + String(c.date.getTime()));
           record(String(c.re) + " " + c.bytes.join() + " " + String(c.bytes.buffer !== o.bytes.buffer));
           record(String(c.list[1][0]));"#,
    );
    assert_eq!(log, ["true", "1true5", "/x+/gi 1,2,3 true", "2"]);
}

#[test]
fn function_throws_data_clone_error() {
    let log = run_ok(r#"record(caught(() => structuredClone({ f() {} })));"#);
    assert_eq!(log, ["DataCloneError/25/true"]);
}

#[test]
fn transfer_detaches_original() {
    let log = run_ok(
        r#"const buf = new Uint8Array([4, 5, 6]).buffer;
           const c = structuredClone({ buf }, { transfer: [buf] });
           record(buf.byteLength + " " + c.buf.byteLength + " " + new Uint8Array(c.buf).join());"#,
    );
    assert_eq!(log, ["0 3 4,5,6"]);
}

#[test]
fn rejects_non_transferable_entry_and_bad_arguments() {
    let log = run_ok(
        r#"record(caught(() => structuredClone({}, { transfer: [{}] })));
           record(caught(() => structuredClone()));
           record(caught(() => structuredClone(1, { transfer: 5 })));
           record(String(structuredClone(1, null)) + String(structuredClone.length));"#,
    );
    assert_eq!(
        log,
        [
            "DataCloneError/25/true",
            "TypeError/undefined/true",
            "TypeError/undefined/true",
            "11"
        ]
    );
}

#[test]
fn op_is_not_visible_to_user_code() {
    let log = run_ok(r#"record(typeof op_structured_clone);"#);
    assert_eq!(log, ["undefined"]);
}

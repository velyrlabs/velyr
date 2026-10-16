//! Runs the timers bootstrap on V8 under the real event loop.
use std::path::PathBuf;
use std::sync::Mutex;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_web_core::{install_timers, timer_host_functions};

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

fn run(script: &str) -> Result<Vec<String>, RunError> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let path: PathBuf = std::env::temp_dir().join(format!(
        "velyr-web-core-{}-{}.js",
        std::process::id(),
        script.len()
    ));
    std::fs::write(&path, script).unwrap();
    let mut host_functions = timer_host_functions();
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
        |isolate, _, timers| install_timers(isolate, timers).expect("timers bootstrap"),
        path.clone(),
    );
    std::fs::remove_file(&path).ok();
    result.map(|()| LOG.lock().unwrap().clone())
}

#[test]
fn sync_then_microtask_then_timer() {
    let log =
        run(r#"setTimeout(() => record("2"), 5); queueMicrotask(() => record("1")); record("0");"#);
    assert_eq!(log.unwrap(), ["0", "1", "2"]);
}

#[test]
fn timeout_receives_extra_args() {
    let log = run(r#"setTimeout((a, b) => record(a + b), 1, "x", "y");"#);
    assert_eq!(log.unwrap(), ["xy"]);
}

#[test]
fn cleared_timeout_does_not_fire() {
    let log = run(r#"const id = setTimeout(() => record("no"), 1); clearTimeout(id);"#);
    assert!(log.unwrap().is_empty());
}

#[test]
fn interval_cleared_on_third_run_fires_three_times() {
    let log = run(
        r#"let n = 0; const id = setInterval(() => { record("t"); if (++n === 3) clearInterval(id); }, 1);"#,
    );
    assert_eq!(log.unwrap(), ["t", "t", "t"]);
}

#[test]
fn non_function_callback_is_a_type_error() {
    let log =
        run(r#"try { queueMicrotask(1); } catch (e) { record(String(e instanceof TypeError)); }"#);
    assert_eq!(log.unwrap(), ["true"]);
}

#[test]
fn timer_ids_are_numbers_and_unique() {
    let log = run(
        r#"const a = setTimeout(() => {}, 1), b = setInterval(() => {}, 1);
           record(typeof a + String(a !== b)); clearInterval(b);"#,
    );
    assert_eq!(log.unwrap(), ["numbertrue"]);
}

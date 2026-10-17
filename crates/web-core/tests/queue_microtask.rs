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
        "velyr-web-core-qm-{}-{}.js",
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
fn queue_microtask_throw_is_not_a_rejection() {
    let log = run(r#"
        globalThis.reportError = (e) => record("reported " + e.message);
        queueMicrotask(() => { throw new Error("boom"); });
        queueMicrotask(() => record("b"));
    "#);
    assert_eq!(log.unwrap(), ["reported boom", "b"]);
}

#[test]
fn sync_then_microtask_then_timer() {
    let log =
        run(r#"setTimeout(() => record("2"), 5); queueMicrotask(() => record("1")); record("0");"#);
    assert_eq!(log.unwrap(), ["0", "1", "2"]);
}

#[test]
fn checkpoint_op_inside_a_reaction_does_not_reenter() {
    let log = run(r#"
        Promise.resolve().then(() => { record("a"); op_microtask_checkpoint(); record("b"); });
        queueMicrotask(() => record("c"));
    "#);
    assert_eq!(log.unwrap(), ["a", "b", "c"]);
}

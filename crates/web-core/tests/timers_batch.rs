//! Batched dispatch of due host timers on V8 under the real event loop.
use std::path::PathBuf;
use std::sync::Mutex;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_resources::QuotaConfig;
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
    run_with(script, None)
}

fn run_with(script: &str, quotas: Option<QuotaConfig>) -> Result<Vec<String>, RunError> {
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
        quotas,
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
fn batch_runs_microtasks_between_callbacks() {
    let log = run(r#"
        setTimeout(() => { record("a"); Promise.resolve().then(() => record("a-then")); }, 1);
        setTimeout(() => record("b"), 1);
        setTimeout(() => record("c"), 1);
        for (const end = Date.now() + 10; Date.now() < end;);
    "#);
    assert_eq!(log.unwrap(), ["a", "a-then", "b", "c"]);
}

#[test]
fn batch_skips_a_timer_cleared_by_an_earlier_callback() {
    let log = run(r#"
        let victim;
        setTimeout(() => { record("a"); clearTimeout(victim); }, 1);
        victim = setTimeout(() => record("victim"), 1);
        setTimeout(() => record("c"), 1);
        for (const end = Date.now() + 10; Date.now() < end;);
    "#);
    assert_eq!(log.unwrap(), ["a", "c"]);
}

#[test]
fn interval_cleared_in_its_own_batched_callback_never_refires() {
    let log = run(r#"
        let n = 0;
        const id = setInterval(() => { record("tick"); if (++n === 1) clearInterval(id); }, 1);
        setTimeout(() => record("other"), 1);
        setTimeout(() => record("end"), 60);
        for (const end = Date.now() + 10; Date.now() < end;);
    "#);
    assert_eq!(log.unwrap(), ["tick", "other", "end"]);
}

#[test]
fn a_throwing_callback_does_not_cancel_the_rest_of_the_batch() {
    let log = run(r#"
        setTimeout(() => { throw new Error("boom"); }, 1);
        setTimeout(() => record("after"), 1);
    "#);
    assert_eq!(log.unwrap(), ["after"]);
}

#[test]
fn nesting_clamp_survives_batching() {
    let log = run(r#"
        const start = Date.now();
        let done = 0;
        function chain(left) {
            if (left === 0) {
                if (++done === 2) record("elapsed " + Math.round(Date.now() - start));
                return;
            }
            setTimeout(() => chain(left - 1), 0);
        }
        chain(12);
        chain(12);
    "#);
    let log = log.unwrap();
    let ms: f64 = log[0].trim_start_matches("elapsed ").parse().unwrap();
    assert!(ms >= 28.0, "12 nested timeouts took {ms} ms");
}

#[test]
fn one_dispatch_for_many_due_timers() {
    let log = run(r#"
        let fired = 0;
        for (let i = 0; i < 1000; i++) setTimeout(() => { fired++; }, 1);
        // Hold the loop until all of them are due together.
        for (const end = Date.now() + 30; Date.now() < end;);
        setTimeout(() => {
            const stats = globalThis[Symbol.for("velyr.timers")].stats();
            record(fired + " " + (stats.batches <= 8) + " " + stats.fired);
        }, 50);
    "#);
    assert_eq!(log.unwrap(), ["1000 true 1000"]);
}

#[test]
fn op_microtask_checkpoint_is_not_reachable_from_user_code() {
    let log = run(r#"record(typeof op_microtask_checkpoint);"#);
    assert_eq!(log.unwrap(), ["undefined"]);
}

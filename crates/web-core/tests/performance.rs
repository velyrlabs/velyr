//! Runs the performance and navigator bootstrap on V8 under the real event loop.
use std::path::PathBuf;
use std::sync::Mutex;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_web_core::{install_performance, performance_host_functions};

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
        "velyr-web-core-perf-{}-{}.js",
        std::process::id(),
        script.len()
    ));
    std::fs::write(&path, script).unwrap();
    let mut host_functions = performance_host_functions();
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
        |isolate, _, _| install_performance(isolate).expect("performance bootstrap"),
        path.clone(),
    );
    std::fs::remove_file(&path).ok();
    result.map(|()| LOG.lock().unwrap().clone())
}

fn run_ok(script: &str) -> Vec<String> {
    run(script).expect("script runs")
}

#[test]
fn now_is_monotonic_and_coarse() {
    let log = run_ok(
        r#"
        let prev = performance.now();
        let ok = prev >= 0;
        let coarse = true;
        for (let i = 0; i < 20000; i++) {
          const t = performance.now();
          if (t < prev) ok = false;
          if (Math.abs(t * 10 - Math.round(t * 10)) > 1e-6) coarse = false;
          prev = t;
        }
        record(String(ok) + " " + String(coarse));
        "#,
    );
    assert_eq!(log, ["true true"]);
}

#[test]
fn now_advances_over_real_time() {
    let log = run_ok(
        r#"
        const start = performance.now();
        while (performance.now() - start < 5) {}
        record("done");
        "#,
    );
    assert_eq!(log, ["done"]);
}

#[test]
fn time_origin_is_epoch_milliseconds() {
    let log = run_ok(
        r#"
        const wall = performance.timeOrigin + performance.now();
        record(String(Math.abs(wall - Date.now()) < 1000));
        "#,
    );
    assert_eq!(log, ["true"]);
}

#[test]
fn measure_between_marks_spans_their_start_times() {
    let log = run_ok(
        r#"
        const a = performance.mark("a");
        while (performance.now() - a.startTime < 2) {}
        const b = performance.mark("b");
        const m = performance.measure("a-to-b", "a", "b");
        record([m.entryType, m.name, m.startTime === a.startTime,
          m.duration === b.startTime - a.startTime, m.duration >= 2].join(" "));
        "#,
    );
    assert_eq!(log, ["measure a-to-b true true true"]);
}

#[test]
fn measure_defaults_run_from_origin_to_now() {
    let log = run_ok(
        r#"
        const m = performance.measure("all");
        record(m.startTime + " " + (m.duration >= 0));
        "#,
    );
    assert_eq!(log, ["0 true"]);
}

#[test]
fn measure_accepts_options_with_start_and_end() {
    let log = run_ok(
        r#"
        performance.mark("a", { startTime: 10 });
        const m = performance.measure("m", { start: "a", end: 25, detail: { k: 1 } });
        record([m.startTime, m.duration, m.detail.k].join(" "));
        "#,
    );
    assert_eq!(log, ["10 15 1"]);
}

#[test]
fn measure_with_unknown_mark_throws() {
    let log = run_ok(
        r#"
        try { performance.measure("m", "missing"); record("no throw"); }
        catch (e) { record(e.name); }
        "#,
    );
    assert_eq!(log, ["SyntaxError"]);
}

#[test]
fn mark_requires_a_name() {
    let log = run_ok(
        r#"
        try { performance.mark(); record("no throw"); }
        catch (e) { record(e.constructor.name); }
        "#,
    );
    assert_eq!(log, ["TypeError"]);
}

#[test]
fn entries_are_listed_filtered_and_cleared() {
    let log = run_ok(
        r#"
        performance.mark("x");
        performance.mark("y");
        performance.mark("x");
        performance.measure("m", "x", "y");
        const names = () => performance.getEntries().map((e) => e.name).join(",");
        record(names());
        record(String(performance.getEntriesByName("x").length));
        record(String(performance.getEntriesByName("m", "mark").length));
        record(String(performance.getEntriesByType("measure").length));
        performance.clearMarks("x");
        record(names());
        performance.clearMarks();
        performance.clearMeasures("m");
        record(names() === "" ? "empty" : names());
        "#,
    );
    assert_eq!(log, ["x,y,x,m", "2", "0", "1", "y,m", "empty"]);
}

#[test]
fn observer_receives_new_entries_asynchronously() {
    let log = run_ok(
        r#"
        const observer = new PerformanceObserver((list, self) => {
          record("cb " + list.getEntries().map((e) => e.name).join(",") + " " + (self === observer));
        });
        observer.observe({ entryTypes: ["mark"] });
        performance.mark("seen");
        performance.measure("ignored");
        record("sync");
        "#,
    );
    assert_eq!(log, ["sync", "cb seen true"]);
}

#[test]
fn observer_stops_after_disconnect() {
    let log = run_ok(
        r#"
        const observer = new PerformanceObserver(() => record("cb"));
        observer.observe({ type: "mark" });
        observer.disconnect();
        performance.mark("late");
        record("done");
        "#,
    );
    assert_eq!(log, ["done"]);
}

#[test]
fn observer_take_records_drains_the_queue() {
    let log = run_ok(
        r#"
        const observer = new PerformanceObserver(() => record("cb"));
        observer.observe({ type: "mark" });
        performance.mark("one");
        record(String(observer.takeRecords().length));
        record(String(observer.takeRecords().length));
        "#,
    );
    assert_eq!(log, ["1", "0"]);
}

#[test]
fn navigator_reports_velyr_identity() {
    let log = run_ok(
        r#"
        record(navigator.userAgent);
        record(navigator.language);
        record(navigator.languages.join(","));
        record(String(Number.isInteger(navigator.hardwareConcurrency) && navigator.hardwareConcurrency >= 1));
        "#,
    );
    assert_eq!(log[0], format!("Velyr/{}", env!("CARGO_PKG_VERSION")));
    assert_eq!(log[1..], ["en-US", "en-US", "true"]);
}

#[test]
fn raw_ops_are_not_visible_to_user_code() {
    let log =
        run_ok(r#"record(Object.keys(globalThis).filter((k) => k.startsWith("op_")).join(","));"#);
    assert_eq!(log, [""]);
}

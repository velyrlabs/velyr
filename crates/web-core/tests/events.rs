//! Runs the Event / EventTarget / DOMException / AbortController bootstrap on V8 under the real event loop.
use std::path::PathBuf;
use std::sync::Mutex;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_structured_clone,
    install_timers, structured_clone_host_functions, timer_host_functions,
};

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
        "velyr-web-core-events-{}-{}.js",
        std::process::id(),
        script.len()
    ));
    std::fs::write(&path, script).unwrap();
    let mut host_functions = timer_host_functions();
    host_functions.extend(encoding_host_functions());
    host_functions.extend(structured_clone_host_functions());
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
        |isolate, _, timers| {
            install_timers(isolate, timers).expect("timers bootstrap");
            install_events(isolate).expect("events bootstrap");
            install_encoding(isolate).expect("encoding bootstrap");
            install_structured_clone(isolate).expect("structured clone bootstrap");
        },
        path.clone(),
    );
    std::fs::remove_file(&path).ok();
    result.map(|()| LOG.lock().unwrap().clone())
}

fn run_ok(script: &str) -> Vec<String> {
    run(script).expect("script runs")
}

#[test]
fn once_listener_runs_a_single_time() {
    let log = run_ok(
        r#"const t = new EventTarget();
           t.addEventListener("x", () => record("hit"), { once: true });
           t.dispatchEvent(new Event("x"));
           t.dispatchEvent(new Event("x"));"#,
    );
    assert_eq!(log, ["hit"]);
}

#[test]
fn event_fields_and_dispatch_result() {
    let log = run_ok(
        r#"const t = new EventTarget();
           const e = new Event("x", { cancelable: true });
           t.addEventListener("x", (ev) => {
             record([ev.type, ev.target === t, ev.currentTarget === t, ev.eventPhase, ev.isTrusted].join());
             ev.preventDefault();
           });
           record(String(t.dispatchEvent(e)));
           record([e.defaultPrevented, e.eventPhase, e.currentTarget].join());
           record(String(new Event("y").cancelable));"#,
    );
    assert_eq!(log, ["x,true,true,2,false", "false", "true,0,", "false"]);
}

#[test]
fn passive_listener_cannot_prevent_default() {
    let log = run_ok(
        r#"const t = new EventTarget();
           t.addEventListener("x", (ev) => ev.preventDefault(), { passive: true });
           record(String(t.dispatchEvent(new Event("x", { cancelable: true }))));"#,
    );
    assert_eq!(log, ["true"]);
}

#[test]
fn handle_event_objects_and_duplicates() {
    let log = run_ok(
        r#"const t = new EventTarget();
           const obj = { handleEvent(ev) { record("obj:" + (this === obj)); } };
           const fn1 = () => record("fn");
           t.addEventListener("x", obj);
           t.addEventListener("x", fn1);
           t.addEventListener("x", fn1);
           t.addEventListener("x", fn1, { capture: true });
           t.dispatchEvent(new Event("x"));
           t.removeEventListener("x", fn1);
           t.dispatchEvent(new Event("x"));"#,
    );
    assert_eq!(log, ["obj:true", "fn", "fn", "obj:true", "fn"]);
}

#[test]
fn stop_immediate_propagation_halts_remaining_listeners() {
    let log = run_ok(
        r#"const t = new EventTarget();
           t.addEventListener("x", (e) => { record("a"); e.stopImmediatePropagation(); });
           t.addEventListener("x", () => record("b"));
           t.dispatchEvent(new Event("x"));"#,
    );
    assert_eq!(log, ["a"]);
}

#[test]
fn listener_added_during_dispatch_is_not_called_and_removed_is_skipped() {
    let log = run_ok(
        r#"const t = new EventTarget();
           const late = () => record("late");
           const victim = () => record("victim");
           t.addEventListener("x", () => { record("first"); t.addEventListener("x", late); t.removeEventListener("x", victim); });
           t.addEventListener("x", victim);
           t.dispatchEvent(new Event("x"));"#,
    );
    assert_eq!(log, ["first"]);
}

#[test]
fn listener_error_does_not_stop_dispatch() {
    let log = run_ok(
        r#"const t = new EventTarget();
           t.addEventListener("x", () => { throw new Error("boom"); });
           t.addEventListener("x", () => record("second"));
           t.dispatchEvent(new Event("x"));"#,
    );
    assert_eq!(log, ["second"]);
}

#[test]
fn signal_option_removes_listener_on_abort() {
    let log = run_ok(
        r#"const t = new EventTarget();
           const c = new AbortController();
           t.addEventListener("x", () => record("hit"), { signal: c.signal });
           t.dispatchEvent(new Event("x"));
           c.abort();
           t.dispatchEvent(new Event("x"));
           const pre = AbortSignal.abort();
           t.addEventListener("x", () => record("never"), { signal: pre });
           t.dispatchEvent(new Event("x"));"#,
    );
    assert_eq!(log, ["hit"]);
}

#[test]
fn invalid_dispatch_arguments_throw() {
    let log = run_ok(
        r#"const t = new EventTarget();
           const err = (f) => { try { f(); return "ok"; } catch (e) { return e.constructor.name; } };
           record(err(() => t.dispatchEvent({})));
           record(err(() => new Event()));
           const e = new Event("x");
           t.addEventListener("x", () => record(err(() => t.dispatchEvent(e))));
           t.dispatchEvent(e);
           t.addEventListener("null", null);"#,
    );
    assert_eq!(log, ["TypeError", "TypeError", "DOMException"]);
}

#[test]
fn custom_event_carries_detail() {
    let log = run_ok(
        r#"const e = new CustomEvent("c", { detail: { n: 1 }, bubbles: true });
           record([e instanceof Event, e.detail.n, e.bubbles, String(new CustomEvent("d").detail)].join());"#,
    );
    assert_eq!(log, ["true,1,true,null"]);
}

#[test]
fn event_target_is_subclassable() {
    let log = run_ok(
        r#"class Bus extends EventTarget {}
           const b = new Bus();
           b.addEventListener("x", () => record("sub"));
           b.dispatchEvent(new Event("x"));"#,
    );
    assert_eq!(log, ["sub"]);
}

#[test]
fn dom_exception_names_and_codes() {
    let log = run_ok(
        r#"const e = new DOMException("msg", "AbortError");
           record([e.name, e.code, e.message, e instanceof Error, DOMException.ABORT_ERR, String(e)].join());
           const d = new DOMException();
           record([d.name, d.code, JSON.stringify(d.message)].join());
           const n = new DOMException("m", "NoSuchThing");
           record([n.name, n.code].join());
           record(String(new DOMException("m", "DataCloneError").code));
           record(String(new DOMException("m", "TimeoutError").code));
           record(Object.prototype.toString.call(e));"#,
    );
    assert_eq!(
        log,
        [
            "AbortError,20,msg,true,20,AbortError: msg",
            "Error,0,\"\"",
            "NoSuchThing,0",
            "25",
            "23",
            "[object DOMException]",
        ]
    );
}

#[test]
fn atob_btoa_and_clone_errors_are_dom_exceptions() {
    let log = run_ok(
        r#"const kind = (f) => { try { f(); return "ok"; } catch (e) { return [e instanceof DOMException, e.name, e.code].join(); } };
           record(kind(() => btoa("Ā")));
           record(kind(() => atob("*")));
           record(kind(() => structuredClone(() => {})));"#,
    );
    assert_eq!(
        log,
        [
            "true,InvalidCharacterError,5",
            "true,InvalidCharacterError,5",
            "true,DataCloneError,25"
        ]
    );
}

#[test]
fn abort_controller_basics() {
    let log = run_ok(
        r#"const c = new AbortController();
           record(String(c.signal.aborted));
           record(String(c.signal.reason));
           c.signal.addEventListener("abort", (e) => record("event:" + e.type));
           c.signal.onabort = () => record("onabort");
           c.abort();
           c.abort();
           record(String(c.signal.aborted));
           record(c.signal.reason.name + "/" + c.signal.reason.code + "/" + (c.signal.reason instanceof DOMException));
           record(String(c.signal instanceof EventTarget));
           const d = new AbortController();
           d.abort("why");
           record(d.signal.reason);
           try { d.signal.throwIfAborted(); } catch (e) { record("threw:" + e); }
           try { new AbortController().signal.throwIfAborted(); record("fine"); } catch (e) { record("bad"); }"#,
    );
    assert_eq!(
        log,
        [
            "false",
            "undefined",
            "event:abort",
            "onabort",
            "true",
            "AbortError/20/true",
            "true",
            "why",
            "threw:why",
            "fine"
        ]
    );
}

#[test]
fn abort_signal_static_abort() {
    let log = run_ok(
        r#"const s = AbortSignal.abort();
           record(s.aborted + "/" + s.reason.name);
           record(AbortSignal.abort("r").reason);
           try { new AbortSignal(); } catch (e) { record(e.constructor.name); }"#,
    );
    assert_eq!(log, ["true/AbortError", "r", "TypeError"]);
}

#[test]
fn abort_signal_timeout_aborts_with_timeout_error() {
    let log = run_ok(
        r#"const s = AbortSignal.timeout(10);
           record(String(s.aborted));
           s.addEventListener("abort", () => record(s.reason.name + "/" + s.reason.code));"#,
    );
    assert_eq!(log, ["false", "TimeoutError/23"]);
}

#[test]
fn abort_signal_any_follows_first_abort() {
    let log = run_ok(
        r#"const a = new AbortController();
           const b = new AbortController();
           const any = AbortSignal.any([a.signal, b.signal]);
           record(String(any.aborted));
           any.addEventListener("abort", () => record("any:" + any.reason));
           b.abort("from-b");
           a.abort("from-a");
           record(any.aborted + "/" + any.reason);
           const pre = AbortSignal.any([AbortSignal.abort("early"), new AbortController().signal]);
           record(pre.aborted + "/" + pre.reason);
           record(String(AbortSignal.any([]).aborted));"#,
    );
    assert_eq!(
        log,
        ["false", "any:from-b", "true/from-b", "true/early", "false"]
    );
}

#[test]
fn abort_signal_any_composes_with_timeout() {
    let log = run_ok(
        r#"const c = new AbortController();
           const any = AbortSignal.any([c.signal, AbortSignal.timeout(10)]);
           any.addEventListener("abort", () => record(any.reason.name));"#,
    );
    assert_eq!(log, ["TimeoutError"]);
}

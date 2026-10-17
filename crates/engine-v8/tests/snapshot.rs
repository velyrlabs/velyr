//! Startup snapshot round trip against real V8 (VELYR-123, VELYR-232).

use std::sync::{Mutex, MutexGuard};

use velyr_engine_api::{
    CallContext, Engine, HostError, HostFunctionDef, Isolate, IsolateOptions, JsString, Source,
    Value,
};
use velyr_engine_v8::{SNAPSHOT_RESULTS_KEY, V8Engine, create_snapshot};

const BOOTSTRAP: &str = "
    class Greeter { hi() { return 'hi'; } }
    globalThis.Greeter = Greeter;
    globalThis.Boot = { answer: () => 40 + 2 };
";

// `create_snapshot` must run alone in its process, but the harness runs tests on threads: every
// test here holds this lock.
static SNAPSHOT_TESTS: Mutex<()> = Mutex::new(());

fn alone() -> MutexGuard<'static, ()> {
    SNAPSHOT_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn src(name: &str, text: &str) -> Source {
    Source {
        name: name.into(),
        text: text.into(),
    }
}

fn text(value: Value) -> String {
    match value {
        Value::String(text) => text.to_string_lossy(),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn op_one(_: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Ok(Value::Number(1.0))
}

fn op_two(_: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Ok(Value::Number(2.0))
}

/// Stands in for a permission check: the answer depends on the calling isolate's own slot.
struct Allowed(bool);

fn op_guarded(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    match ctx.isolate_slot::<Allowed>() {
        Some(Allowed(true)) => Ok(ctx.create_string(JsString::from("granted"))),
        _ => Err(HostError::TypeError("denied".into())),
    }
}

fn def(name: &'static str, callback: velyr_engine_api::HostFn) -> HostFunctionDef {
    HostFunctionDef {
        name,
        arity: 0,
        callback,
    }
}

fn ops() -> Vec<HostFunctionDef> {
    vec![
        def("op_one", op_one),
        def("op_two", op_two),
        def("op_guarded", op_guarded),
    ]
}

/// Captures and hides `op_two` and `op_guarded`, as the real bootstraps do (ADR-010).
const CAPTURE: &str = "
    const two = globalThis.op_two;
    const guarded = globalThis.op_guarded;
    delete globalThis.op_two;
    delete globalThis.op_guarded;
    globalThis.callTwo = () => two();
    globalThis.callGuarded = () => { try { return guarded(); } catch (e) { return e.message; } };
";

fn booted(blob: &'static [u8], ops: Vec<HostFunctionDef>) -> velyr_engine_v8::V8Isolate {
    V8Engine::with_snapshot(blob, ops)
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

#[test]
fn snapshot_boots_isolate_with_bootstrap_globals() {
    let _alone = alone();
    let blob = create_snapshot(&[], &[src("<bootstrap>", BOOTSTRAP)]).expect("snapshot");
    let mut isolate = booted(blob.leak(), Vec::new());
    let probe = src("probe.js", "new Greeter().hi() === 'hi' && Boot.answer()");
    assert_eq!(
        isolate.eval_script(&probe).expect("eval"),
        Value::Number(42.0)
    );
}

#[test]
fn snapshot_isolate_without_blob_has_no_bootstrap_globals() {
    let _alone = alone();
    let mut isolate = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let probe = src("probe.js", "typeof Boot");
    assert_eq!(
        isolate.eval_script(&probe).expect("eval"),
        Value::String(JsString::from("undefined"))
    );
}

#[test]
fn snapshot_creation_reports_bootstrap_exception() {
    let _alone = alone();
    let err = create_snapshot(&[], &[src("<bootstrap>", "throw new Error('boom')")])
        .expect_err("throwing bootstrap");
    assert!(err.to_string().contains("boom"), "{err}");
}

#[test]
fn snapshot_boots_isolates_on_parallel_threads() {
    let _alone = alone();
    let blob: &'static [u8] = create_snapshot(&[], &[src("<bootstrap>", BOOTSTRAP)])
        .expect("snapshot")
        .leak();
    let workers: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(move || {
                booted(blob, Vec::new())
                    .eval_script(&src("probe.js", "Boot.answer()"))
                    .expect("eval")
            })
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().expect("worker"), Value::Number(42.0));
    }
}

#[test]
fn snapshot_ops_captured_by_the_bootstrap_reach_their_callbacks() {
    let _alone = alone();
    let blob = create_snapshot(&ops(), &[src("velyr:capture", CAPTURE)]).expect("snapshot");
    let mut isolate = booted(blob.leak(), ops());
    let probe = src(
        "probe.js",
        "[callTwo(), op_one(), typeof op_two, typeof op_guarded].join()",
    );
    assert_eq!(
        text(isolate.eval_script(&probe).expect("eval")),
        "2,1,undefined,undefined"
    );
}

#[test]
fn snapshot_ops_answer_with_the_calling_isolates_own_slots() {
    let _alone = alone();
    let blob: &'static [u8] = create_snapshot(&ops(), &[src("velyr:capture", CAPTURE)])
        .expect("snapshot")
        .leak();
    // One isolate at a time: a thread's isolates cannot be used interleaved.
    let answer = |allowed: bool| {
        let mut isolate = booted(blob, ops());
        isolate.set_slot(Allowed(allowed));
        text(
            isolate
                .eval_script(&src("probe.js", "callGuarded()"))
                .expect("eval"),
        )
    };
    assert_eq!(answer(true), "granted");
    assert_eq!(answer(false), "denied");
}

#[test]
fn snapshot_boot_rejects_an_op_list_that_differs_from_the_blob() {
    let _alone = alone();
    let blob: &'static [u8] = create_snapshot(&ops(), &[src("velyr:capture", CAPTURE)])
        .expect("snapshot")
        .leak();
    let mut reordered = ops();
    reordered.swap(0, 1);
    let mut missing = ops();
    missing.pop();
    let mut extra = ops();
    extra.push(def("op_extra", op_one));
    for list in [reordered, missing, extra, Vec::new()] {
        let names: Vec<_> = list.iter().map(|def| def.name).collect();
        let err = V8Engine::with_snapshot(blob, list)
            .create_isolate(IsolateOptions::default())
            .err()
            .unwrap_or_else(|| panic!("{names:?} booted"));
        assert!(err.to_string().contains("op"), "{err}");
    }
}

#[test]
fn snapshot_bootstrap_frames_keep_the_script_name() {
    let _alone = alone();
    let bootstrap = "globalThis.fail = function fail() { throw new Error('inside'); };";
    let blob = create_snapshot(&[], &[src("velyr:failing", bootstrap)]).expect("snapshot");
    let mut isolate = booted(blob.leak(), Vec::new());
    let probe = src("probe.js", "try { fail(); } catch (e) { e.stack }");
    let stack = text(isolate.eval_script(&probe).expect("eval"));
    assert!(stack.contains("velyr:failing:1"), "{stack}");
}

#[test]
fn snapshot_keeps_each_scripts_completion_value_for_the_embedder() {
    let _alone = alone();
    let scripts = [
        src("velyr:answer", "40 + 2"),
        src("velyr:nothing", "void 0"),
    ];
    let blob = create_snapshot(&[], &scripts).expect("snapshot");
    let mut isolate = booted(blob.leak(), Vec::new());
    let probe = format!(
        "const r = globalThis[Symbol.for({SNAPSHOT_RESULTS_KEY:?})];
         [Object.getPrototypeOf(r), r['velyr:answer'], 'velyr:nothing' in r].join()"
    );
    assert_eq!(
        text(isolate.eval_script(&src("probe.js", &probe)).expect("eval")),
        ",42,false"
    );
}

#[test]
fn snapshot_isolate_registers_more_host_functions_after_boot() {
    let _alone = alone();
    let blob = create_snapshot(&ops(), &[src("velyr:capture", CAPTURE)]).expect("snapshot");
    let mut isolate = booted(blob.leak(), ops());
    isolate
        .register_host_function(def("op_late", op_two))
        .expect("register");
    let probe = src("probe.js", "[op_late(), op_one(), callTwo()].join()");
    assert_eq!(text(isolate.eval_script(&probe).expect("eval")), "2,1,2");
}

#[test]
fn snapshot_engine_reports_that_it_boots_from_a_snapshot() {
    let _alone = alone();
    let blob = create_snapshot(&[], &[]).expect("snapshot");
    assert!(V8Engine::with_snapshot(blob.leak(), Vec::new()).boots_from_snapshot());
    assert!(!V8Engine::new().boots_from_snapshot());
}

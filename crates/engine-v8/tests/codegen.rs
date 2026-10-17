//! Code generation from strings switch (`eval`, `new Function`) against real V8.

use std::sync::{Mutex, MutexGuard};

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_v8::{V8Engine, V8Isolate, create_snapshot};

// `create_snapshot` must run alone in its process, so every test holds this lock.
static SERIAL: Mutex<()> = Mutex::new(());

fn alone() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn isolate(engine: &V8Engine) -> V8Isolate {
    engine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn src(text: &str) -> Source {
    Source {
        name: "probe.js".into(),
        text: text.into(),
    }
}

#[test]
fn codegen_from_strings_is_rejected_when_disabled() {
    let _alone = alone();
    let mut iso = isolate(&V8Engine::new().disallow_code_generation_from_strings());
    for code in ["eval('1')", "new Function('return 1')"] {
        let err = iso.eval_script(&src(code)).expect_err(code);
        assert!(err.message.contains("EvalError"), "{code}: {}", err.message);
    }
}

#[test]
fn codegen_from_strings_allowed_by_default() {
    let _alone = alone();
    let mut iso = isolate(&V8Engine::new());
    let probe = src("eval('1') + new Function('return 1')()");
    assert_eq!(iso.eval_script(&probe).expect("eval"), Value::Number(2.0));
}

#[test]
fn snapshot_engine_keeps_the_flag_off_by_default() {
    let _alone = alone();
    let blob = create_snapshot(&[], &[src("globalThis.ready = true;")]).expect("snapshot");
    let mut iso = isolate(&V8Engine::with_snapshot(blob.leak(), Vec::new()));
    let probe = src("ready && eval('1') === 1 && new Function('return 1')() === 1");
    assert_eq!(iso.eval_script(&probe).expect("eval"), Value::Bool(true));
}

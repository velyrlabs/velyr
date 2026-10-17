//! Startup snapshot round trip against real V8 (VELYR-123).

use std::sync::{Mutex, MutexGuard};

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_v8::{V8Engine, create_snapshot};

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

#[test]
fn snapshot_boots_isolate_with_bootstrap_globals() {
    let _alone = alone();
    let blob = create_snapshot(&[src("<bootstrap>", BOOTSTRAP)]).expect("snapshot");
    let mut isolate = V8Engine::with_snapshot(blob.leak())
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
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
    let err = create_snapshot(&[src("<bootstrap>", "throw new Error('boom')")])
        .expect_err("throwing bootstrap");
    assert!(err.to_string().contains("boom"), "{err}");
}

#[test]
fn snapshot_boots_isolates_on_parallel_threads() {
    let _alone = alone();
    let blob: &'static [u8] = create_snapshot(&[src("<bootstrap>", BOOTSTRAP)])
        .expect("snapshot")
        .leak();
    let workers: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(move || {
                let mut isolate = V8Engine::with_snapshot(blob)
                    .create_isolate(IsolateOptions::default())
                    .expect("isolate");
                isolate
                    .eval_script(&src("probe.js", "Boot.answer()"))
                    .expect("eval")
            })
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().expect("worker"), Value::Number(42.0));
    }
}

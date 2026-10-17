//! Heap stats are sampled on a 250 ms interval, not on every turn (RT-14).

use std::sync::{Arc, Mutex};

use velyr_engine_api::{Isolate, Source};
use velyr_engine_v8::{V8Engine, V8Isolate};
use velyr_runtime_core::{LoopIsolate, RuntimeConfig, metrics, run_file};

const WORKER_ID: usize = 98;

fn run_script<T: Send + 'static>(
    worker: usize,
    script: &str,
    probe: impl FnOnce(&mut LoopIsolate<V8Isolate>) -> T + Send + 'static,
) -> T {
    let path = std::env::temp_dir().join(format!(
        "velyr-heap-sampling-{}-{worker}.js",
        std::process::id()
    ));
    std::fs::write(&path, script).unwrap();
    let out = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&out);
    let result = run_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            ..RuntimeConfig::default()
        },
        Vec::new(),
        "",
        move |isolate, _, _| {
            // Own worker slot: the main one is shared with the other tests of this binary.
            metrics::global().register_worker(worker);
            *slot.lock().unwrap() = Some(probe(isolate));
        },
        path.clone(),
    );
    std::fs::remove_file(&path).unwrap();
    result.unwrap();
    out.lock().unwrap().take().expect("probe ran")
}

#[test]
fn heap_sampling_is_rate_limited() {
    let samples = run_script(97, "0;", |isolate| {
        let source = Source {
            name: "turn.js".into(),
            text: "0".into(),
        };
        for _ in 0..1000 {
            isolate.eval_script(&source).expect("eval");
        }
        isolate.heap_samples()
    });
    assert!(
        (1..=3).contains(&samples),
        "{samples} heap samples in 1000 turns"
    );
}

#[test]
fn final_sample_taken_on_exit() {
    // The only turn that allocates is the entry script, inside the sampling interval.
    run_script(
        WORKER_ID,
        "globalThis.keep = new Uint8Array(32 << 20);",
        |isolate| {
            let source = Source {
                name: "first.js".into(),
                text: "0".into(),
            };
            isolate.eval_script(&source).expect("eval");
        },
    );
    let heap = metrics::global()
        .snapshot()
        .workers
        .iter()
        .find(|w| w.id == WORKER_ID)
        .and_then(|w| w.heap)
        .expect("a heap sample");
    assert!(
        heap.used_bytes + heap.external_bytes.unwrap_or(0) >= 32 << 20,
        "{heap:?}"
    );
}

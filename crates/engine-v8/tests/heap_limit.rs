use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source};
use velyr_engine_v8::V8Engine;

const HEAP_LIMIT_BYTES: usize = 32 * 1024 * 1024;

fn limited_isolate() -> impl Isolate {
    V8Engine::new()
        .create_isolate(IsolateOptions {
            heap_limit_bytes: Some(HEAP_LIMIT_BYTES),
        })
        .expect("isolate")
}

fn src(text: &str) -> Source {
    Source {
        name: "heap.js".into(),
        text: text.into(),
    }
}

#[test]
fn endless_allocation_ends_with_a_heap_quota_error_and_the_process_survives() {
    let mut iso = limited_isolate();
    let err = iso
        .eval_script(&src(
            "const keep = []; while (true) keep.push(new Array(100000).fill(1));",
        ))
        .expect_err("allocation must be stopped");
    assert_eq!(err.quota_class(), Some("heap"), "{err:?}");
}

#[test]
fn plain_termination_is_not_reported_as_a_quota_error() {
    let mut iso = limited_isolate();
    iso.termination_handle().terminate();
    let err = iso
        .eval_script(&src("while (true) {}"))
        .expect_err("terminated");
    assert_eq!(err.quota_class(), None, "{err:?}");
}

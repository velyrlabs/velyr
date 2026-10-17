//! The event loop's idle GC (VELYR-215) decides from `used_bytes` whether the heap grew since
//! its last collection, so a requested GC must lower it before `request_gc` returns.

use velyr_engine_api::{Engine, GcReason, Isolate, IsolateOptions, Source};
use velyr_engine_v8::V8Engine;

#[test]
fn request_gc_lowers_used_heap_before_returning() {
    let mut iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let garbage = Source {
        name: "garbage.js".into(),
        // ~30 MB promoted to old space, then dropped.
        text: "globalThis.junk = Array.from({ length: 300000 }, (_, i) => ({ i, s: 'x' + i }));
               globalThis.junk = null;"
            .into(),
    };
    iso.eval_script(&garbage).expect("garbage");
    let before = iso.heap_stats().used_bytes;
    iso.request_gc(GcReason::Idle);
    let after = iso.heap_stats().used_bytes;
    assert!(
        after + (8 << 20) < before,
        "used heap {} MB -> {} MB",
        before >> 20,
        after >> 20
    );
}

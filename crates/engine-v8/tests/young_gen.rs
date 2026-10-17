//! The young generation is capped (`--max-semi-space-size=16`, VELYR-212): churning short-lived
//! garbage must not grow the heap by the ~55 MB per isolate that V8's default 64 MB semi-space
//! allowed. Its own test binary, so the process-wide flags are the ones `init_platform` sets.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source};
use velyr_engine_v8::V8Engine;

#[test]
fn young_garbage_churn_keeps_the_heap_small() {
    let mut iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let before = iso.heap_stats().total_bytes;
    let churn = Source {
        name: "churn.js".into(),
        // ~400 MB of short-lived objects, each batch dropped before the next.
        text: "let keep = 0;
               for (let round = 0; round < 400; round++) {
                 const batch = [];
                 for (let i = 0; i < 10000; i++) batch.push({ i, s: 'x' + i, a: [i, i] });
                 keep += batch.length;
               }
               keep"
            .into(),
    };
    iso.eval_script(&churn).expect("churn");
    let grown = iso.heap_stats().total_bytes.saturating_sub(before);
    assert!(
        grown < 40 << 20,
        "heap grew {} MB under young churn",
        grown >> 20
    );
}

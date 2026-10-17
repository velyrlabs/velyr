//! Prelude install cost per isolate (measurement only, N01). Run with:
//! `cargo test -p velyr-engine-native --test prelude_cost --release -- --ignored --nocapture`

use std::time::Instant;

use velyr_engine_api::{Engine, Isolate, IsolateOptions};
use velyr_engine_native::{
    NativeEngine, bytecode_compiler::compile_named_script, iterator_helpers,
};
use velyr_engine_vm::interpreter::Interpreter;

const ITERATIONS: usize = 200;
const ROUNDS: usize = 5;
const PRELUDES: [(&str, &str); 7] = [
    (
        "temporal",
        include_str!("../src/builtins/temporal/prelude.js"),
    ),
    (
        "temporal_plain",
        include_str!("../src/builtins/temporal/prelude_plain.js"),
    ),
    (
        "temporal_zoned",
        include_str!("../src/builtins/temporal/prelude_zoned.js"),
    ),
    ("intl", include_str!("../src/builtins/intl/prelude.js")),
    (
        "intl_number",
        include_str!("../src/builtins/intl/prelude_number.js"),
    ),
    (
        "intl_formats",
        include_str!("../src/builtins/intl/prelude_formats.js"),
    ),
    ("iterator_helpers", ""),
];

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Mean milliseconds per call over `ITERATIONS`, median across `ROUNDS`.
fn mean_ms(mut body: impl FnMut()) -> f64 {
    let rounds = (0..ROUNDS)
        .map(|_| {
            let start = Instant::now();
            (0..ITERATIONS).for_each(|_| body());
            start.elapsed().as_secs_f64() * 1000.0 / ITERATIONS as f64
        })
        .collect();
    median(rounds)
}

#[test]
#[ignore]
fn prelude_cost() {
    let full = mean_ms(|| {
        drop(
            NativeEngine
                .create_isolate(IsolateOptions::default())
                .unwrap(),
        )
    });
    let bare = mean_ms(|| drop(Interpreter::new()));
    println!("isolate full  : {full:.3} ms (median of {ROUNDS} rounds x {ITERATIONS})");
    println!("isolate bare  : {bare:.3} ms");
    println!("isolate delta : {:.3} ms", full - bare);

    let iterator_helpers_run = mean_ms(|| {
        let mut vm = Interpreter::new();
        iterator_helpers::install(&mut vm).unwrap();
    });
    println!("bare + iterator_helpers (compile+run): {iterator_helpers_run:.3} ms");

    // The Intl/Temporal preludes need crate-private native ops, so they can only be compiled
    // here; their run cost is the remainder of the full delta.
    let mut compile_total = 0.0;
    for (name, source) in PRELUDES.iter().filter(|(_, source)| !source.is_empty()) {
        let compile = mean_ms(|| drop(compile_named_script("velyr:p", source).unwrap()));
        compile_total += compile;
        println!(
            "compile {name:<16}: {compile:.3} ms ({} bytes)",
            source.len()
        );
    }
    println!("compile total (6 preludes): {compile_total:.3} ms");

    let bare_heap = Interpreter::new().heap_usage().0;
    let mut helpers_vm = Interpreter::new();
    iterator_helpers::install(&mut helpers_vm).unwrap();
    let helpers_heap = helpers_vm.heap_usage().0;
    let full_heap = NativeEngine
        .create_isolate(IsolateOptions::default())
        .unwrap()
        .heap_stats()
        .used_bytes;
    println!("heap bare            : {bare_heap} B");
    println!("heap + iter helpers  : {helpers_heap} B");
    println!("heap full isolate    : {full_heap} B");
    println!(
        "heap delta (full-bare): {} B ({} KiB)",
        full_heap - bare_heap,
        (full_heap - bare_heap) / 1024
    );
}

//! `heap_limit_bytes` is a hard limit on the native isolate's GC heap (VELYR-182): a
//! script that allocates without end fails with the `heap` quota error instead of
//! taking the whole process down, and the host keeps running.

use velyr_engine_api::{Engine, GcReason, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

const LIMIT_BYTES: usize = 32 << 20;

fn isolate() -> NativeIsolate {
    let options = IsolateOptions {
        heap_limit_bytes: Some(LIMIT_BYTES),
    };
    NativeEngine.create_isolate(options).expect("isolate")
}

fn eval(iso: &mut NativeIsolate, text: &str) -> Result<Value, String> {
    let source = Source {
        name: "heap_limit.js".into(),
        text: text.into(),
    };
    iso.eval_script(&source)
        .map_err(|error| error.quota_class().unwrap_or(&error.message).to_string())
}

#[test]
fn endless_object_allocation_fails_with_the_heap_quota() {
    let mut iso = isolate();
    let result = eval(
        &mut iso,
        "const kept = []; for (;;) kept.push({ n: kept.length });",
    );
    assert_eq!(result.err().as_deref(), Some("heap"));
}

#[test]
fn endless_string_growth_fails_with_the_heap_quota() {
    let mut iso = isolate();
    let script = "const kept = []; for (let i = 0; ; i++) kept.push('x'.repeat(1000) + i);";
    assert_eq!(eval(&mut iso, script).err().as_deref(), Some("heap"));
}

#[test]
fn endless_array_growth_fails_with_the_heap_quota() {
    let mut iso = isolate();
    let script = "const kept = []; for (let i = 0; ; i++) kept.push(i, i, i, i);";
    assert_eq!(eval(&mut iso, script).err().as_deref(), Some("heap"));
}

#[test]
fn one_huge_string_fails_with_the_heap_quota_before_it_is_built() {
    let mut iso = isolate();
    assert_eq!(
        eval(&mut iso, "'x'.repeat(2 ** 29)").err().as_deref(),
        Some("heap")
    );
}

#[test]
fn heap_stats_count_strings_and_elements_against_the_limit() {
    let mut iso = isolate();
    let before = iso.heap_stats();
    eval(
        &mut iso,
        "globalThis.kept = ['y'.repeat(1 << 20), new Array(1 << 16).fill(0)];",
    )
    .expect("fits the limit");
    let after = iso.heap_stats();
    let limit = after.limit_bytes.expect("limited isolate");
    assert!(limit >= LIMIT_BYTES);
    assert!(after.used_bytes >= before.used_bytes + (1 << 20) + (1 << 16) * 8);
    assert!(after.used_bytes <= limit);
}

#[test]
fn a_huge_fill_fails_with_the_heap_quota() {
    let mut iso = isolate();
    let script = "new Array(2 ** 32 - 1).fill(0)";
    assert_eq!(eval(&mut iso, script).err().as_deref(), Some("heap"));
}

/// Runs `setup` (which must fit the quota), then `script`, which must hit it.
fn setup_then_fails_with_heap(setup: &str, script: &str) {
    let mut iso = isolate();
    eval(&mut iso, setup).expect("setup fits the limit");
    assert_eq!(eval(&mut iso, script).err().as_deref(), Some("heap"));
}

#[test]
fn a_huge_copy_within_fails_with_the_heap_quota() {
    setup_then_fails_with_heap(
        "globalThis.a = new Array(1 << 22); a.fill(0, 0, 1 << 21);",
        "a.copyWithin(1 << 21, 0)",
    );
}

#[test]
fn a_huge_splice_fails_with_the_heap_quota() {
    setup_then_fails_with_heap(
        "globalThis.a = new Array(1 << 21).fill(0);",
        "a.splice(0)",
    );
}

#[test]
fn a_huge_array_from_fails_with_the_heap_quota() {
    setup_then_fails_with_heap(
        "globalThis.a = new Array(1 << 21).fill(0);",
        "Array.from(a)",
    );
}

#[test]
fn a_huge_array_like_array_from_fails_with_the_heap_quota() {
    let script = "Array.from({ length: 1 << 23 })";
    assert_eq!(eval(&mut isolate(), script).err().as_deref(), Some("heap"));
}

#[test]
fn a_huge_join_fails_with_the_heap_quota() {
    setup_then_fails_with_heap(
        "globalThis.a = new Array(1 << 19);",
        "a.join('x'.repeat(256))",
    );
}

#[test]
fn a_huge_concat_fails_with_the_heap_quota() {
    setup_then_fails_with_heap(
        "globalThis.a = new Array(1 << 21).fill(0);",
        "a.concat(a, a, a)",
    );
}

#[test]
fn a_map_with_endless_unique_keys_fails_with_the_heap_quota() {
    let mut iso = isolate();
    let script = "const m = new Map(); for (let i = 0; ; i++) m.set(i, i);";
    assert_eq!(eval(&mut iso, script).err().as_deref(), Some("heap"));
}

#[test]
fn objects_with_endless_unique_keys_fail_with_the_heap_quota() {
    let mut iso = isolate();
    // Each object is garbage right away; only the shapes and atoms of its key stay.
    let script = "for (let i = 0; ; i++) { const o = {}; o['k' + i] = i; }";
    assert_eq!(eval(&mut iso, script).err().as_deref(), Some("heap"));
}

#[test]
fn endless_unique_shapes_over_few_keys_fail_with_the_heap_quota() {
    let mut iso = isolate();
    // 24 keys, one subset per i: no new atoms, only new shapes.
    let script = "for (let i = 0; ; i++) { const o = {};
        for (let b = 0; b < 24; b++) if (i & (1 << b)) o['k' + b] = 0; }";
    assert_eq!(eval(&mut iso, script).err().as_deref(), Some("heap"));
}

#[test]
fn isolates_on_one_thread_have_their_own_budgets() {
    let mut first = isolate();
    let mut second = isolate();
    // 28 MiB here and 10 MiB in the second isolate, in 1 MiB strings: `repeat` reserves
    // two bytes per unit and a Latin-1 string keeps one.
    let keep = |mib: u32| {
        format!(
            "globalThis.kept = []; for (let i = 0; i < {mib}; i++) kept.push('x'.repeat(1 << 20) + i); 0"
        )
    };
    eval(&mut first, &keep(28)).expect("fits its own limit");
    let result = eval(&mut second, &keep(10));
    assert_eq!(result, Ok(Value::Number(0.0)));
}

#[test]
fn churn_near_the_limit_does_not_collect_on_every_allocation() {
    let mut iso = isolate();
    eval(&mut iso, "globalThis.kept = []; 0").expect("fits the limit");
    // Fills up to about 8 KiB of headroom, so every few short-lived arrays reach the
    // limit. `repeat(n)` reserves 2n bytes and keeps n, so each step takes half the room.
    loop {
        iso.request_gc(GcReason::Explicit);
        let stats = iso.heap_stats();
        let room = stats.limit_bytes.expect("limited isolate") - stats.used_bytes;
        if room < 8 * 1024 {
            break;
        }
        let pad = format!("kept.push('x'.repeat({})); 0", (room - 4096) / 2);
        eval(&mut iso, &pad).expect("fits the limit");
    }
    let before = iso.heap_stats().gc_count.unwrap();
    eval(&mut iso, "for (let i = 0; i < 20000; i++) [i]; 0").expect("garbage is reclaimed");
    let collections = iso.heap_stats().gc_count.unwrap() - before;
    assert!(collections < 100, "{collections} collections");
}

/// The name of the error `code` throws, or `no error`.
fn thrown_name(code: &str) -> String {
    let probe = format!(
        "(() => {{ try {{ {code}; return 'no error'; }} catch (e) {{ return e.name; }} }})()"
    );
    match eval(&mut isolate(), &probe) {
        Ok(Value::String(name)) => name.to_string_lossy(),
        other => panic!("{code}: {other:?}"),
    }
}

#[test]
fn huge_array_like_typed_array_is_a_range_error_before_reading_elements() {
    assert_eq!(
        thrown_name("new Int8Array({ length: 2 ** 53 })"),
        "RangeError"
    );
    assert_eq!(
        thrown_name("new BigInt64Array({ length: 2 ** 53 })"),
        "RangeError"
    );
}

#[test]
fn huge_array_like_argument_list_is_a_range_error() {
    let f = "function () {}";
    assert_eq!(
        thrown_name(&format!("Reflect.apply({f}, null, {{ length: 2 ** 53 }})")),
        "RangeError"
    );
    assert_eq!(
        thrown_name(&format!("Reflect.construct({f}, {{ length: 2 ** 32 }})")),
        "RangeError"
    );
}

#[test]
fn huge_array_like_apply_is_a_range_error() {
    let f = "function () {}";
    assert_eq!(
        thrown_name(&format!("({f}).apply(null, {{ length: 2 ** 53 }})")),
        "RangeError"
    );
    assert_eq!(
        thrown_name(&format!(
            "Function.prototype.apply.call({f}, null, {{ length: 2 ** 32 }})"
        )),
        "RangeError"
    );
}

/// Accounting cost probe, not a correctness test: run in release with
/// `--ignored --exact accounting_throughput_probe --nocapture`.
#[test]
#[ignore = "benchmark"]
fn accounting_throughput_probe() {
    let script = "let s = ''; for (let i = 0; i < 1e5; i++) s += 'ab' + i; s.charCodeAt(0);
        const a = []; for (let i = 0; i < 1e6; i++) a.push(i); a.length";
    let mut best = std::time::Duration::MAX;
    for _ in 0..7 {
        let mut iso = NativeEngine
            .create_isolate(IsolateOptions {
                heap_limit_bytes: Some(1 << 30),
            })
            .expect("isolate");
        let start = std::time::Instant::now();
        assert_eq!(eval(&mut iso, script), Ok(Value::Number(1e6)));
        best = best.min(start.elapsed());
    }
    eprintln!("accounting_throughput_probe best of 7: {best:?}");
}

#[test]
fn a_fresh_isolate_runs_after_one_hit_the_limit() {
    let mut iso = isolate();
    let script = "const kept = []; for (;;) kept.push([kept.length]);";
    assert!(eval(&mut iso, script).is_err());
    drop(iso);
    assert_eq!(eval(&mut isolate(), "1 + 1"), Ok(Value::Number(2.0)));
}

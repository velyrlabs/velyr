use std::path::PathBuf;
use std::time::{Duration, Instant};

use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, Runtime, RuntimeConfig};

fn script(name: &str, text: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("velyr-quota-{name}-{}.js", std::process::id()));
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn cpu_budget_stops_an_infinite_loop_with_a_cpu_quota_error() {
    let path = script("cpu", "while (true) {}");
    let mut isolate = Runtime::<V8Engine>::new(RuntimeConfig {
        cpu_budget_ms: Some(100),
        ..RuntimeConfig::default()
    })
    .create_isolate()
    .unwrap();
    let started = Instant::now();
    let result = isolate.run_script(&path);
    let elapsed = started.elapsed();
    std::fs::remove_file(&path).unwrap();
    assert!(
        matches!(result, Err(RunError::QuotaExceeded { class: "cpu" })),
        "{result:?}"
    );
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

#[test]
fn script_within_the_cpu_budget_is_not_terminated() {
    let path = script("fast", "1 + 1");
    let mut isolate = Runtime::<V8Engine>::new(RuntimeConfig {
        cpu_budget_ms: Some(1000),
        ..RuntimeConfig::default()
    })
    .create_isolate()
    .unwrap();
    let result = isolate.run_script(&path);
    std::fs::remove_file(&path).unwrap();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn manual_termination_stays_terminated_even_with_a_cpu_budget() {
    let path = script("manual", "while (true) {}");
    let mut isolate = Runtime::<V8Engine>::new(RuntimeConfig {
        cpu_budget_ms: Some(60_000),
        ..RuntimeConfig::default()
    })
    .create_isolate()
    .unwrap();
    let handle = isolate.termination_handle();
    let killer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        handle.terminate();
    });
    let result = isolate.run_script(&path);
    killer.join().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(result, Err(RunError::Terminated)), "{result:?}");
}

#[test]
fn heap_limit_stops_endless_allocation_with_a_heap_quota_error() {
    let path = script(
        "heap",
        "const keep = []; while (true) keep.push(new Array(100000).fill(1));",
    );
    let mut isolate = Runtime::<V8Engine>::new(RuntimeConfig {
        heap_limit_bytes: Some(32 * 1024 * 1024),
        ..RuntimeConfig::default()
    })
    .create_isolate()
    .unwrap();
    let result = isolate.run_script(&path);
    std::fs::remove_file(&path).unwrap();
    assert!(
        matches!(result, Err(RunError::QuotaExceeded { class: "heap" })),
        "{result:?}"
    );
}

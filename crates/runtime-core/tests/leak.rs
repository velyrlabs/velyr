use std::path::PathBuf;

use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_resources::{IsolateId, LeakEntry, QuotaClass, ResourceId, ResourceState};

fn fake_leak() -> Vec<LeakEntry> {
    vec![LeakEntry {
        id: ResourceId {
            index: 3,
            generation: 0,
        },
        owner: IsolateId(1),
        kind: "test_file",
        quota_class: QuotaClass::Files,
        capability: None,
        span_id: None,
        state: ResourceState::Open,
    }]
}

fn run(
    name: &str,
    leak_check: bool,
    collector: Option<fn() -> Vec<LeakEntry>>,
) -> Result<(), RunError> {
    let path: PathBuf =
        std::env::temp_dir().join(format!("velyr-leak-{name}-{}.js", std::process::id()));
    std::fs::write(&path, "1 + 1").unwrap();
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        leak_check,
        leak_collector: collector,
        ..RuntimeConfig::default()
    };
    let result = run_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        |_, _, _| {},
        path.clone(),
    );
    std::fs::remove_file(&path).unwrap();
    result
}

#[test]
fn leak_check_fails_the_run_and_lists_the_open_resource() {
    let err = run("fail", true, Some(fake_leak)).unwrap_err();
    assert!(
        matches!(err, RunError::Leak(ref entries) if entries.len() == 1),
        "{err:?}"
    );
    assert!(err.to_string().contains("test_file"), "{err}");
}

#[test]
fn leaks_are_ignored_without_the_flag() {
    assert!(run("ignored", false, Some(fake_leak)).is_ok());
}

#[test]
fn leak_check_passes_when_nothing_is_open() {
    assert!(run("clean", true, Some(Vec::new)).is_ok());
}

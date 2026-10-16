use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file, run_module_file};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_runtime_resources::{IsolateId, LeakEntry, QuotaClass, ResourceId, ResourceState};

const TEST_TIMEOUT: Duration = Duration::from_secs(20);
const PENDING_TIMER: Duration = Duration::from_secs(600);

fn script(name: &str, text: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("velyr-terminate-{name}-{}.mjs", std::process::id()));
    std::fs::write(&path, text).unwrap();
    path
}

fn fake_leak() -> Vec<LeakEntry> {
    vec![LeakEntry {
        id: ResourceId {
            index: 0,
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

/// Runs `path` with a ref'd timer pending and cancels the shutdown token after 100 ms.
fn run_cancelled(
    path: PathBuf,
    module: bool,
    collector: Option<fn() -> Vec<LeakEntry>>,
) -> Result<(), RunError> {
    let token = CancellationToken::new();
    let dir = std::fs::canonicalize(path.parent().unwrap()).unwrap();
    let capabilities = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            dir.to_str().unwrap(),
        )))
        .build();
    let config = RuntimeConfig {
        capabilities: Some(capabilities),
        platform_init: Some(V8Engine::init_platform),
        shutdown: Some(token.clone()),
        leak_check: collector.is_some(),
        leak_collector: collector,
        ..RuntimeConfig::default()
    };
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        let install = |_: &mut _, _, timers: velyr_runtime_event_loop::TimerHandle| {
            timers.set(PENDING_TIMER, false);
        };
        let result = if module {
            run_module_file(V8Engine::new(), config, Vec::new(), "", install, path)
        } else {
            run_file(V8Engine::new(), config, Vec::new(), "", install, path)
        };
        let _ = done_tx.send(result);
    });
    thread::sleep(Duration::from_millis(100));
    token.cancel();
    done_rx
        .recv_timeout(TEST_TIMEOUT)
        .expect("run did not stop after the shutdown token was cancelled")
}

#[test]
fn cancelled_token_stops_a_script_with_a_pending_timer() {
    let path = script("script", "1 + 1");
    let result = run_cancelled(path.clone(), false, None);
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(result, Err(RunError::Terminated)), "{result:?}");
}

#[test]
fn cancelled_token_stops_a_module_with_a_pending_timer() {
    let path = script("module", "await new Promise(() => {});");
    let result = run_cancelled(path.clone(), true, None);
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(result, Err(RunError::Terminated)), "{result:?}");
}

#[test]
fn terminated_run_that_still_leaks_reports_the_leak() {
    let path = script("leak", "1 + 1");
    let result = run_cancelled(path.clone(), false, Some(fake_leak));
    std::fs::remove_file(&path).unwrap();
    assert!(
        matches!(result, Err(RunError::Leak(ref entries)) if entries.len() == 1),
        "{result:?}"
    );
}

#[test]
fn terminated_run_without_leaks_is_terminated() {
    let path = script("clean", "1 + 1");
    let result = run_cancelled(path.clone(), false, Some(Vec::new));
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(result, Err(RunError::Terminated)), "{result:?}");
}

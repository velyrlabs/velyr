//! A worker costs its own thread plus the isolate watchdog, not an extra joining thread.
// Counting threads needs /proc or `ps`, neither of which exists on Windows.
#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig};
use velyr_runtime_ops::test_host_functions;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_runtime_resources::{QuotaConfig, QuotaContext};
use velyr_runtime_workers::{ParentContext, WorkerHandle, WorkerOptions};

const WAIT: Duration = Duration::from_secs(20);
const WORKERS: usize = 8;

/// Both tests count process threads, so they must not overlap.
static SERIAL: Mutex<()> = Mutex::new(());

fn thread_count() -> usize {
    #[cfg(target_os = "linux")]
    return std::fs::read_dir("/proc/self/task").unwrap().count();
    // `ps -M` prints one row per thread (plus a header).
    #[cfg(not(target_os = "linux"))]
    return {
        let out = std::process::Command::new("ps")
            .args(["-M", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).lines().count() - 1
    };
}

fn module(name: &str, text: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("velyr-tc-{name}-{}.mjs", std::process::id()));
    std::fs::write(&path, text).unwrap();
    dunce::canonicalize(path).unwrap()
}

fn parent() -> ParentContext {
    let dir = dunce::canonicalize(std::env::temp_dir()).unwrap();
    let import = Capability::ImportLocal(PathPattern::new(dir.to_str().unwrap()));
    let capabilities = CapabilitySet::builder().allow(import).build();
    ParentContext {
        capabilities: capabilities.clone(),
        deployment_policy: capabilities,
        quotas: Arc::new(QuotaContext::new(QuotaConfig {
            workers: Some(WORKERS as u64),
            ..QuotaConfig::default()
        })),
        heap_limit_bytes: None,
    }
}

fn spawn(entry: PathBuf, install: fn()) -> WorkerHandle {
    let parent = parent();
    let options = WorkerOptions {
        requested: parent.capabilities.clone(),
        entry,
        heap_limit_bytes: None,
        config: RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            ..RuntimeConfig::default()
        },
        host_functions: test_host_functions(),
        bootstrap_js: "",
    };
    WorkerHandle::spawn(V8Engine::new(), &parent, options, move |_, _, _| install()).unwrap()
}

#[test]
fn each_worker_has_no_extra_joining_thread() {
    let _guard = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let entry = module(
        "idle",
        "op_worker_post_message(1); await op_worker_recv_message();",
    );
    // Warm up the process-wide V8 platform threads so only per-worker threads are counted.
    let _ = spawn(entry.clone(), || {}).terminate(WAIT);
    let before = thread_count();
    let workers: Vec<_> = (0..WORKERS).map(|_| spawn(entry.clone(), || {})).collect();
    // Let every worker reach its steady state so no thread is still being created.
    std::thread::sleep(Duration::from_secs(2));
    let delta = thread_count() - before;
    for worker in workers {
        let _ = worker.terminate(WAIT);
    }
    // worker thread + watchdog monitor each; the joining "velyr-isolate" thread made it 3.
    assert!(
        delta <= 2 * WORKERS,
        "{WORKERS} workers added {delta} threads"
    );
}

#[test]
fn a_panicking_worker_reports_an_exit_error() {
    let _guard = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let worker = spawn(module("panic", ""), || panic!("boom"));
    assert!(matches!(worker.join(WAIT), Err(RunError::Setup(_))));
}

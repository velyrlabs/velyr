use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use velyr_engine_api::{Engine, Isolate, IsolateOptions, SerializedValue, Source, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig};
use velyr_runtime_errors::ErrorCode;
use velyr_runtime_ops::test_host_functions;
use velyr_runtime_permissions::{Capability, CapabilitySet, NamePattern, PathPattern, Request};
use velyr_runtime_resources::{QuotaClass, QuotaConfig, QuotaContext};
use velyr_runtime_workers::{ParentContext, WorkerHandle, WorkerOptions};

const WAIT: Duration = Duration::from_secs(20);

fn module(name: &str, text: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("velyr-worker-{name}-{}.mjs", std::process::id()));
    std::fs::write(&path, text).unwrap();
    std::fs::canonicalize(path).unwrap()
}

fn import_dir() -> Capability {
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    Capability::ImportLocal(PathPattern::new(dir.to_str().unwrap()))
}

fn caps(extra: Vec<Capability>) -> Arc<CapabilitySet> {
    extra
        .into_iter()
        .fold(CapabilitySet::builder().allow(import_dir()), |b, c| {
            b.allow(c)
        })
        .build()
}

fn parent(capabilities: Arc<CapabilitySet>, workers: u64) -> ParentContext {
    ParentContext {
        capabilities,
        deployment_policy: caps(vec![
            Capability::FsRead(PathPattern::new("/")),
            Capability::EnvRead(NamePattern::new("*")),
        ]),
        quotas: Arc::new(QuotaContext::new(QuotaConfig {
            workers: Some(workers),
            ..QuotaConfig::default()
        })),
        heap_limit_bytes: None,
    }
}

fn options(entry: PathBuf, requested: Arc<CapabilitySet>, leak_check: bool) -> WorkerOptions {
    WorkerOptions {
        entry,
        requested,
        heap_limit_bytes: None,
        config: RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            leak_check,
            ..RuntimeConfig::default()
        },
        host_functions: test_host_functions(),
        bootstrap_js: "",
    }
}

fn spawn(parent: &ParentContext, options: WorkerOptions) -> WorkerHandle {
    WorkerHandle::spawn(V8Engine::new(), parent, options, |_, _, _| {}).unwrap()
}

/// Next message from the worker, failing the test after `WAIT`.
fn recv(worker: &mut WorkerHandle) -> SerializedValue {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async { tokio::time::timeout(WAIT, worker.recv_message()).await })
        .expect("worker sent nothing in time")
        .expect("worker channel closed")
}

const PENDING: &str = "op_worker_post_message(1); await op_worker_recv_message();";

#[test]
fn effective_authority_is_the_intersection_with_the_parent() {
    let parent = parent(
        caps(vec![
            Capability::FsRead(PathPattern::new("/a")),
            Capability::EnvRead(NamePattern::new("PATH")),
        ]),
        4,
    );
    let requested = caps(vec![
        Capability::FsRead(PathPattern::new("/")),
        Capability::EnvRead(NamePattern::new("*")),
    ]);
    let entry = module("authority", PENDING);
    let worker = spawn(&parent, options(entry.clone(), requested.clone(), false));
    let effective = worker.capabilities();
    assert!(effective.check(&Request::FsRead("/a/file")).is_allow());
    assert!(!effective.check(&Request::FsRead("/etc/passwd")).is_allow());
    assert!(!effective.check(&Request::FsRead("/")).is_allow());
    assert!(matches!(worker.terminate(WAIT), Err(RunError::Terminated)));

    let allowed = module("env-allowed", "nativeEnvRead('PATH');");
    let worker = spawn(&parent, options(allowed.clone(), requested.clone(), false));
    let result = worker.join(WAIT);
    assert!(result.is_ok(), "{result:?}");

    let denied = module("env-denied", "nativeEnvRead('HOME');");
    let worker = spawn(&parent, options(denied.clone(), requested, false));
    let result = worker.join(WAIT);
    assert!(
        matches!(&result, Err(RunError::Uncaught(e)) if e.message.contains("PERMISSION")),
        "{result:?}"
    );
    for path in [entry, allowed, denied] {
        std::fs::remove_file(path).unwrap();
    }
}

static PROCESS_GUARDS: std::sync::Mutex<Vec<velyr_runtime_resources::QuotaGuard>> =
    std::sync::Mutex::new(Vec::new());
static PROCESS_OUTCOMES: std::sync::Mutex<Vec<bool>> = std::sync::Mutex::new(Vec::new());

/// `testReserveProcesses(n)`: reserves `n` `processes` units from the calling isolate's quota
/// and keeps them, as a spawned child would.
fn test_reserve_processes(
    ctx: &mut dyn velyr_engine_api::CallContext,
    args: &[Value],
) -> Result<Value, velyr_engine_api::HostError> {
    let Some(Value::Number(n)) = args.first() else {
        return Err(velyr_engine_api::HostError::TypeError("a count".into()));
    };
    let op_ctx = ctx
        .isolate_slot::<velyr_runtime_ops::OpContext>()
        .ok_or_else(|| velyr_engine_api::HostError::TypeError("no OpContext".into()))?;
    let reserved = op_ctx.quotas.reserve(QuotaClass::Processes, *n as u64);
    let ok = reserved.is_ok();
    PROCESS_GUARDS.lock().unwrap().extend(reserved.ok());
    PROCESS_OUTCOMES.lock().unwrap().push(ok);
    Ok(Value::Undefined)
}

#[test]
fn the_processes_quota_covers_all_workers_together() {
    let parent = ParentContext {
        quotas: Arc::new(QuotaContext::new(QuotaConfig {
            processes: Some(3),
            ..QuotaConfig::default()
        })),
        ..parent(caps(vec![]), 4)
    };
    let entry = module("processes", &format!("testReserveProcesses(2); {PENDING}"));
    let open = || {
        let mut opts = options(entry.clone(), caps(vec![]), false);
        opts.host_functions.push(velyr_engine_api::HostFunctionDef {
            name: "testReserveProcesses",
            arity: 1,
            callback: test_reserve_processes,
        });
        opts
    };
    let mut first = spawn(&parent, open());
    recv(&mut first);
    let mut second = spawn(&parent, open());
    recv(&mut second);
    // Two workers asking for two units each: with a per-isolate quota both would succeed.
    assert_eq!(*PROCESS_OUTCOMES.lock().unwrap(), [true, false]);
    assert_eq!(parent.quotas.used(QuotaClass::Processes), 2);
    PROCESS_GUARDS.lock().unwrap().clear();
    let _ = first.terminate(WAIT);
    let _ = second.terminate(WAIT);
    std::fs::remove_file(entry).unwrap();
}

#[test]
fn third_worker_exceeds_a_quota_of_two() {
    let parent = parent(caps(vec![]), 2);
    let entry = module("quota", PENDING);
    let open = || options(entry.clone(), caps(vec![]), false);
    let first = spawn(&parent, open());
    let second = spawn(&parent, open());
    let err = WorkerHandle::spawn(V8Engine::new(), &parent, open(), |_, _, _| {})
        .err()
        .expect("third worker must be denied");
    assert_eq!(err.code(), ErrorCode::QuotaExceeded);
    assert_eq!(parent.quotas.used(QuotaClass::Workers), 2);

    let _ = first.terminate(WAIT);
    let _ = second.terminate(WAIT);
    assert_eq!(parent.quotas.used(QuotaClass::Workers), 0);
    let _ = spawn(&parent, open()).terminate(WAIT);
    std::fs::remove_file(entry).unwrap();
}

#[test]
fn terminate_with_a_pending_op_closes_resources() {
    let parent = parent(caps(vec![]), 2);
    let entry = module(
        "terminate",
        &format!("nativeLeakOpen(); nativeLeakOpen(); {PENDING}"),
    );
    let mut worker = spawn(&parent, options(entry.clone(), caps(vec![]), true));
    recv(&mut worker);
    // With leak_check, a terminated run that still held resources fails with `Leak`.
    let result = worker.terminate(WAIT);
    std::fs::remove_file(entry).unwrap();
    assert!(matches!(result, Err(RunError::Terminated)), "{result:?}");
}

#[test]
fn terminate_interrupts_running_js() {
    let parent = parent(caps(vec![]), 2);
    let entry = module("busy", "op_worker_post_message(1); while (true) {}");
    let mut worker = spawn(&parent, options(entry.clone(), caps(vec![]), false));
    recv(&mut worker);
    let result = worker.terminate(WAIT);
    std::fs::remove_file(entry).unwrap();
    assert!(matches!(result, Err(RunError::Terminated)), "{result:?}");
}

#[test]
fn normal_exit_with_an_open_resource_still_reports_the_leak() {
    let parent = parent(caps(vec![]), 2);
    let entry = module("leak", "nativeLeakOpen();");
    let worker = spawn(&parent, options(entry.clone(), caps(vec![]), true));
    let result = worker.join(WAIT);
    std::fs::remove_file(entry).unwrap();
    assert!(
        matches!(&result, Err(RunError::Leak(entries)) if entries.len() == 1),
        "{result:?}"
    );
}

#[test]
fn messages_round_trip_as_structured_clones() {
    let parent = parent(caps(vec![]), 2);
    let entry = module(
        "echo",
        "while (await op_worker_recv_message()) { op_worker_post_message(op_worker_take_message()); }",
    );
    let mut worker = spawn(&parent, options(entry.clone(), caps(vec![]), false));
    V8Engine::init_platform();
    let mut isolate = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .unwrap();
    let eval = |isolate: &mut <V8Engine as Engine>::Isolate, text: &str| {
        isolate
            .eval_script(&Source {
                name: "<test>".into(),
                text: text.into(),
            })
            .unwrap()
    };
    let message = eval(&mut isolate, "({ n: 42 })");
    worker
        .post_message(isolate.serialize(&message, &[]).unwrap())
        .unwrap();
    let echoed = isolate.deserialize(&recv(&mut worker)).unwrap();
    let read_n = eval(&mut isolate, "(o) => o.n");
    let n = isolate.call(&read_n, &Value::Undefined, &[echoed]).unwrap();
    assert_eq!(n, Value::Number(42.0));
    let _ = worker.terminate(WAIT);
    std::fs::remove_file(entry).unwrap();
}

#[test]
fn transferred_buffers_are_detached_in_the_worker() {
    let parent = parent(caps(vec![]), 2);
    let entry = module(
        "transfer",
        "const b = new ArrayBuffer(8); op_worker_post_message(b, [b]); op_worker_post_message(b.byteLength); await op_worker_recv_message();",
    );
    let mut worker = spawn(&parent, options(entry.clone(), caps(vec![]), false));
    V8Engine::init_platform();
    let mut isolate = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .unwrap();
    let buffer = isolate.deserialize(&recv(&mut worker)).unwrap();
    let detached_len = isolate.deserialize(&recv(&mut worker)).unwrap();
    let byte_length = isolate
        .eval_script(&Source {
            name: "<test>".into(),
            text: "(b) => b.byteLength".into(),
        })
        .unwrap();
    let len = isolate
        .call(&byte_length, &Value::Undefined, &[buffer])
        .unwrap();
    assert_eq!(len, Value::Number(8.0));
    assert_eq!(detached_len, Value::Number(0.0));
    let _ = worker.terminate(WAIT);
    std::fs::remove_file(entry).unwrap();
}

const MIB: usize = 1 << 20;

/// Polls until the parent's `workers` quota is free again (the worker thread has exited).
fn wait_quota_released(parent: &ParentContext) -> bool {
    let deadline = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < deadline {
        if parent.quotas.used(QuotaClass::Workers) == 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn worker_heap_is_capped_at_the_parent_limit() {
    let mut parent = parent(caps(vec![]), 4);
    parent.heap_limit_bytes = Some(64 * MIB);
    let entry = module("heap", PENDING);
    let open = |heap| WorkerOptions {
        heap_limit_bytes: heap,
        ..options(entry.clone(), caps(vec![]), false)
    };
    let capped = spawn(&parent, open(Some(256 * MIB)));
    assert_eq!(capped.heap_limit_bytes(), Some(64 * MIB));
    let smaller = spawn(&parent, open(Some(32 * MIB)));
    assert_eq!(smaller.heap_limit_bytes(), Some(32 * MIB));
    let inherited = spawn(&parent, open(None));
    assert_eq!(inherited.heap_limit_bytes(), Some(64 * MIB));
    parent.heap_limit_bytes = None;
    let unbounded_parent = spawn(&parent, open(Some(256 * MIB)));
    assert_eq!(unbounded_parent.heap_limit_bytes(), Some(256 * MIB));
    for worker in [capped, smaller, inherited, unbounded_parent] {
        let _ = worker.terminate(WAIT);
    }
    std::fs::remove_file(entry).unwrap();
}

#[test]
fn worker_that_exits_on_its_own_releases_its_quota() {
    let parent = parent(caps(vec![]), 1);
    let done = module("done", "1 + 1;");
    let pending = module("after-done", PENDING);
    let finished = spawn(&parent, options(done.clone(), caps(vec![]), false));
    assert!(
        wait_quota_released(&parent),
        "finished worker kept its quota"
    );
    let next = spawn(&parent, options(pending.clone(), caps(vec![]), false));
    let _ = next.terminate(WAIT);
    drop(finished);
    for path in [done, pending] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn dropping_the_handle_stops_a_busy_worker_thread() {
    let parent = parent(caps(vec![]), 1);
    let entry = module("drop-busy", "op_worker_post_message(1); while (true) {}");
    let mut worker = spawn(&parent, options(entry.clone(), caps(vec![]), false));
    recv(&mut worker);
    drop(worker);
    // The quota guard lives on the worker thread, so its release proves the thread exited.
    let stopped = wait_quota_released(&parent);
    std::fs::remove_file(entry).unwrap();
    assert!(
        stopped,
        "worker thread still alive after the handle was dropped"
    );
}

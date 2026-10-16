//! The web messaging API (`MessageChannel`, `Worker`) on V8 under the real event loop, with a
//! minimal host: no timers, no CLI.

use std::sync::{Arc, Mutex};

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_runtime_resources::QuotaConfig;
use velyr_runtime_workers::{
    ParentContext, WorkerHandle, WorkerHost, WorkerOptions, WorkerScope, install_web_api,
    web_api_host_functions,
};
use velyr_web_core::{
    install_events, install_structured_clone, install_url, structured_clone_host_functions,
    url_host_functions,
};

// `run_file` runs on its own thread, so the probe records into a process-wide log.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());

fn record(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("record expects a string".into()));
    };
    LOG.lock().unwrap().push(text.to_string_lossy());
    Ok(Value::Undefined)
}

fn functions() -> Vec<HostFunctionDef> {
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(web_api_host_functions());
    functions.extend(structured_clone_host_functions());
    functions.extend(url_host_functions());
    functions.push(HostFunctionDef {
        name: "record",
        arity: 1,
        callback: record,
    });
    functions
}

fn config(permissions: &Arc<CapabilitySet>) -> RuntimeConfig {
    RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        capabilities: Some(permissions.clone()),
        ..RuntimeConfig::default()
    }
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("velyr-web-api-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(dir).unwrap()
}

fn host(permissions: &Arc<CapabilitySet>) -> WorkerHost {
    WorkerHost {
        deployment_policy: permissions.clone(),
        heap_limit_bytes: None,
        spawn: Arc::new(|parent: &ParentContext, request| {
            let options = WorkerOptions {
                entry: request.entry,
                requested: parent.capabilities.clone(),
                heap_limit_bytes: None,
                config: config(&parent.capabilities),
                host_functions: functions(),
                bootstrap_js: BOOTSTRAP_JS,
            };
            let scope_name = request.name;
            let group = request.group;
            WorkerHandle::spawn(V8Engine::new(), parent, options, move |isolate, _, _| {
                install_url(isolate).unwrap();
                install_events(isolate).unwrap();
                install_structured_clone(isolate).unwrap();
                let scope = WorkerScope {
                    name: &scope_name,
                    group,
                };
                install_web_api(isolate, None, Some(scope)).unwrap();
            })
        }),
    }
}

/// Runs `body` as an async script on a main isolate that may spawn workers from `dir`.
fn run(dir: &std::path::Path, name: &str, body: &str) -> Vec<String> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let script = format!(
        "(async () => {{ {body} }})().catch((e) => record('FAIL ' + (e && e.stack || e)));"
    );
    let path = dir.join(format!("{name}.js"));
    std::fs::write(&path, script).unwrap();
    let permissions = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            dir.to_str().unwrap(),
        )))
        .build();
    let (spawner, quotas) = (host(&permissions), QuotaConfig::default());
    let caps = permissions.clone();
    let result = run_file(
        V8Engine::new(),
        config(&permissions),
        functions(),
        BOOTSTRAP_JS,
        move |isolate, sender, _timers| {
            install_op_context(isolate, 0, sender, caps, quotas);
            install_url(isolate).unwrap();
            install_events(isolate).unwrap();
            install_structured_clone(isolate).unwrap();
            install_web_api(isolate, Some(spawner), None).unwrap();
        },
        path,
    );
    result.expect("script runs");
    LOG.lock().unwrap().clone()
}

#[test]
fn message_channel_round_trips_a_clone() {
    let dir = temp_dir();
    let log = run(
        &dir,
        "channel",
        "const { port1, port2 } = new MessageChannel();
         port2.onmessage = (e) => { record(JSON.stringify(e.data)); port2.close(); };
         const sent = { n: 1, list: [1, 2] };
         port1.postMessage(sent);
         sent.n = 2;",
    );
    assert_eq!(log, ["{\"n\":1,\"list\":[1,2]}"]);
}

#[test]
fn full_queue_fails_the_sender_with_a_quota_error() {
    let dir = temp_dir();
    let log = run(
        &dir,
        "full",
        "const { port1, port2 } = new MessageChannel();
         let sent = 0;
         try { for (; sent < 20000; sent++) port1.postMessage(sent); }
         catch (error) { record(error.code + ' ' + sent); }
         port1.close(); port2.close();",
    );
    assert_eq!(log, ["VELYR_QUOTA_EXCEEDED 10000"]);
}

#[test]
fn terminate_stops_a_busy_worker_and_the_main_isolate_goes_on() {
    let dir = temp_dir();
    std::fs::write(
        dir.join("busy.js"),
        "self.postMessage('ready'); for (;;) {}",
    )
    .unwrap();
    let log = run(
        &dir,
        "terminate",
        &format!(
            "const worker = new Worker('file://{}/busy.js');
             await new Promise((resolve) => {{ worker.onmessage = resolve; }});
             worker.terminate();
             record('terminated');",
            dir.display()
        ),
    );
    assert_eq!(log, ["terminated"]);
}

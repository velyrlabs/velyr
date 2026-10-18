//! Host for the `node:worker_threads` tests: the wiring the CLI gives the main isolate and every
//! worker (timers, URL, encoding, structured clone, the web messaging API, CommonJS, compat-node),
//! over `NodeBuiltinResolver` and the CommonJS source transform so a worker entry can be ESM or
//! CommonJS. Scripts report through `record`, which appends to a process-wide log.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use velyr_compat_commonjs::{CommonJsTransform, commonjs_host_functions, install_commonjs};
use velyr_compat_node::{
    NodeBuiltinResolver, ProcessConfig, compat_node_host_functions, configure_process,
    install_buffer_global, install_compat_node, install_process_global,
};
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Isolate, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{
    LoopIsolate, ModuleResolverHandle, RunError, RuntimeConfig, run_module_file,
};
use velyr_runtime_event_loop::TimerHandle;
use velyr_runtime_modules::FsResolver;
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_runtime_resources::QuotaConfig;
use velyr_runtime_workers::{
    ParentContext, WorkerHandle, WorkerHost, WorkerOptions, WorkerScope, install_web_api,
    web_api_host_functions, worker_exit_hook,
};
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_structured_clone,
    install_timers, install_url, structured_clone_host_functions, timer_host_functions,
    url_host_functions,
};
use velyr_web_streams::{flate_host_functions, install_web_streams};

const WATCHDOG: Duration = Duration::from_secs(60);

// The main isolate and the workers run on their own threads, so they record into one log.
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
    functions.extend(timer_host_functions());
    functions.extend(url_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(web_api_host_functions());
    functions.extend(structured_clone_host_functions());
    functions.extend(commonjs_host_functions());
    functions.extend(compat_node_host_functions());
    functions.push(HostFunctionDef {
        name: "record",
        arity: 1,
        callback: record,
    });
    functions
}

fn config(permissions: &Arc<CapabilitySet>, resolver: &ModuleResolverHandle) -> RuntimeConfig {
    RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        capabilities: Some(permissions.clone()),
        module_resolver: Some(resolver.clone()),
        source_transform: Some(Arc::new(CommonJsTransform::new(permissions.clone()))),
        ..RuntimeConfig::default()
    }
}

fn install_all<I: Isolate>(
    isolate: &mut LoopIsolate<I>,
    timers: TimerHandle,
    host: WorkerHost,
    scope: Option<WorkerScope<'_>>,
    permissions: Arc<CapabilitySet>,
) {
    if scope.is_some() {
        // `process.exit(n)` in a worker ends that worker with code n.
        let config = ProcessConfig {
            on_exit: worker_exit_hook(),
            ..ProcessConfig::default()
        };
        configure_process(isolate, config);
    }
    install_timers(isolate, timers.clone()).expect("timers");
    install_url(isolate).expect("url");
    install_events(isolate).expect("events");
    install_encoding(isolate).expect("encoding");
    install_web_streams(isolate).expect("web streams");
    install_structured_clone(isolate).expect("structured clone");
    install_web_api(isolate, Some(host), scope).expect("web api");
    install_commonjs(isolate, permissions).expect("commonjs");
    install_compat_node(isolate).expect("compat node");
    install_buffer_global(isolate).expect("buffer");
    let drain = install_process_global(isolate, timers).expect("process global");
    isolate.set_tick_hook(drain);
}

fn worker_host(policy: Arc<CapabilitySet>, resolver: ModuleResolverHandle) -> WorkerHost {
    let spawn_policy = policy.clone();
    WorkerHost {
        deployment_policy: policy,
        heap_limit_bytes: None,
        spawn: Arc::new(move |parent: &ParentContext, request| {
            let requested = parent.capabilities.clone();
            let host = worker_host(spawn_policy.clone(), resolver.clone());
            let options = WorkerOptions {
                entry: request.entry,
                requested: requested.clone(),
                heap_limit_bytes: request.heap_limit_bytes,
                config: config(&requested, &resolver),
                host_functions: functions(),
                bootstrap_js: BOOTSTRAP_JS,
            };
            let (name, group) = (request.name, request.group);
            WorkerHandle::spawn(
                V8Engine::new(),
                parent,
                options,
                move |isolate, _, timers| {
                    let scope = WorkerScope { name: &name, group };
                    install_all(isolate, timers, host, Some(scope), requested);
                },
            )
        }),
    }
}

/// A scratch directory with the facades the builtin resolver needs, both granted for import.
pub struct Fixture {
    pub dir: PathBuf,
    _work: tempfile::TempDir,
    _facades: tempfile::TempDir,
    resolver: NodeBuiltinResolver,
}

impl Fixture {
    pub fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let facades = tempfile::tempdir().unwrap();
        let resolver = NodeBuiltinResolver::new(
            Arc::new(FsResolver),
            &dunce::canonicalize(facades.path()).unwrap(),
        )
        .unwrap();
        Self {
            dir: dunce::canonicalize(work.path()).unwrap(),
            _work: work,
            _facades: facades,
            resolver,
        }
    }

    pub fn write(&self, name: &str, text: &str) -> &Self {
        std::fs::write(self.dir.join(name), text).unwrap();
        self
    }

    /// The scratch directory with forward slashes, safe to embed in JS string literals on Windows.
    pub fn slash_dir(&self) -> String {
        self.dir.display().to_string().replace('\\', "/")
    }

    pub fn url(&self, name: &str) -> String {
        let dir = self.slash_dir();
        // A Windows drive path (`C:/...`) needs a leading slash to form `file:///C:/...`.
        let lead = if dir.starts_with('/') { "" } else { "/" };
        format!("file://{lead}{}/{name}", dir.trim_end_matches('/'))
    }

    fn permissions(&self) -> Arc<CapabilitySet> {
        let allow = |path: &Path| Capability::ImportLocal(PathPattern::new(path.to_str().unwrap()));
        CapabilitySet::builder()
            .allow(allow(&self.dir))
            .allow(allow(self.resolver.facade_dir()))
            .build()
    }

    /// Runs `main.mjs` (written by the caller) and returns what the scripts recorded, or the
    /// failure text. Panics when it does not finish before the watchdog.
    pub fn run(&self) -> Result<Vec<String>, String> {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        LOG.lock().unwrap().clear();
        let permissions = self.permissions();
        let resolver: ModuleResolverHandle = Arc::new(self.resolver.clone());
        let host = worker_host(permissions.clone(), resolver.clone());
        let entry = self.dir.join("main.mjs");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let caps = permissions.clone();
            let result = run_module_file(
                V8Engine::new(),
                config(&permissions, &resolver),
                functions(),
                BOOTSTRAP_JS,
                move |isolate, sender, timers| {
                    install_op_context(isolate, 0, sender, caps.clone(), QuotaConfig::default());
                    install_all(isolate, timers, host, None, caps);
                },
                entry,
            );
            let _ = tx.send(result);
        });
        let result = rx
            .recv_timeout(WATCHDOG)
            .expect("the script did not finish before the watchdog");
        match result {
            Ok(()) => Ok(LOG.lock().unwrap().clone()),
            Err(RunError::UnhandledRejection(reason)) => Err(reason),
            Err(RunError::Uncaught(exception)) => Err(exception.message),
            Err(other) => Err(format!("{other:?}")),
        }
    }
}

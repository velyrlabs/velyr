//! What each profile (`profile-minimal|web|node|full`) installs on a fresh isolate.
//!
//! Timers, `URL`, events/abort, text encoding, streams, the file APIs and net are part of every profile, `minimal`
//! included: the kernel needs the first three and they pull in no network code; the file and net APIs stay default-deny.
//! Streams are pure JS plus gzip/deflate ops and need no permissions, so they are not gated either; the same goes for
//! WebCrypto (`crypto`), which only computes over bytes the script already holds.
//! Web crates (fetch, WebSocket, HTTP/TLS) sit behind `cfg(feature = "web")` and the npm resolver behind
//! `cfg(feature = "npm")`; `profile-minimal` links neither (see the features in Cargo.toml).

mod bootstrap;

use std::sync::Arc;
use velyr_engine_api::Isolate as _;
use velyr_engine_api::{
    Engine, EngineFeatures, HostFunctionDef, JsException, JsString, Source, Value,
};
use velyr_engine_v8::{SNAPSHOT_RESULTS_KEY, V8Engine};
#[cfg(feature = "web")]
use velyr_io_http::{HttpMetrics, set_http_metrics};
use velyr_io_net::{install_net_slots, system_resolver};
#[cfg(feature = "web")]
use velyr_io_tls::install_tls_slots;
use velyr_runtime_core::metrics::{self, install_metrics};
use velyr_runtime_core::{LoopIsolate, ModuleResolverHandle};
use velyr_runtime_event_loop::{CompletionSender, TimerHandle};

use velyr_runtime_core::RuntimeConfig;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_ops::{BOOTSTRAP_JS, OpContext, take_leak_report};
use velyr_runtime_permissions::CapabilitySet;
use velyr_runtime_resources::{QuotaConfig, QuotaContext};
use velyr_runtime_workers::{
    ParentContext, SpawnRequest, WorkerHandle, WorkerHost, WorkerOptions, WorkerScope,
    install_web_api_slots, start_web_api, worker_host_functions,
};
use velyr_web_core::{
    install_blob_slots, install_encoding_slots, install_performance_slots, install_timer_slots,
};
#[cfg(feature = "web")]
use velyr_web_fetch::install_web_fetch_slots;
use velyr_web_streams::install_web_streams_slots;

pub use bootstrap::host_functions;

use crate::permissions::Permissions;

/// The startup snapshot `build.rs` baked for this profile; empty when the build could not
/// (cross-compiling).
static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/startup.snapshot"));

/// The engine `velyr run` and its workers use: booting from the startup snapshot unless the
/// build has none or `VELYR_NO_SNAPSHOT=1`.
pub fn v8_engine() -> V8Engine {
    let disabled = std::env::var_os("VELYR_NO_SNAPSHOT").is_some_and(|value| value == "1");
    let engine = if SNAPSHOT.is_empty() || disabled {
        V8Engine::new()
    } else {
        V8Engine::with_snapshot(SNAPSHOT, host_functions(V8Engine::new().features()))
    };
    if hardening().disallow_code_generation {
        engine.disallow_code_generation_from_strings()
    } else {
        engine
    }
}

/// Opt-in CLI hardening, process-wide so workers inherit it (set once from `main`).
#[derive(Clone, Copy, Default)]
pub struct Hardening {
    pub disallow_code_generation: bool,
    pub frozen_intrinsics: bool,
}

static HARDENING: std::sync::OnceLock<Hardening> = std::sync::OnceLock::new();

pub fn set_hardening(hardening: Hardening) {
    let _ = HARDENING.set(hardening);
}

fn hardening() -> Hardening {
    HARDENING.get().copied().unwrap_or_default()
}

const FREEZE_INTRINSICS: &str = "for (const intrinsic of [Object, Array, Function, String, Promise]) Object.freeze(intrinsic.prototype);";

fn freeze_intrinsics<I: velyr_engine_api::Isolate>(isolate: &mut LoopIsolate<I>) {
    let source = Source {
        name: "velyr:frozen-intrinsics".into(),
        text: FREEZE_INTRINSICS.into(),
    };
    isolate.eval_script(&source).expect("the freeze script is valid JS");
}

/// How a run's isolates start.
#[derive(Clone, Copy)]
pub struct Boot {
    pub features: EngineFeatures,
    /// The startup snapshot already holds the host functions and bootstrap scripts.
    pub from_snapshot: bool,
}

impl Boot {
    pub fn of(engine: &impl Engine) -> Self {
        Self {
            features: engine.features(),
            from_snapshot: engine.boots_from_snapshot(),
        }
    }

    /// What runtime-core registers and evaluates before `install`: nothing the snapshot holds.
    pub fn bootstrap(&self) -> (Vec<HostFunctionDef>, &'static str) {
        if self.from_snapshot {
            return (Vec::new(), "");
        }
        (host_functions(self.features), BOOTSTRAP_JS)
    }
}

/// Feeds io-http's request reports into the runtime metrics.
#[cfg(feature = "web")]
struct HttpSink;

#[cfg(feature = "web")]
impl HttpMetrics for HttpSink {
    fn request_finished(&self, status: u16, elapsed: std::time::Duration) {
        metrics::global().record_http_request(status, elapsed);
    }
}

/// `velyr run` hosts a single isolate.
const ISOLATE_ID: u64 = 0;

/// Runs on the main isolate's thread after the op bootstrap and before user code.
pub fn install<I: velyr_engine_api::Isolate>(
    isolate: &mut LoopIsolate<I>,
    sender: CompletionSender,
    timers: TimerHandle,
    permissions: Arc<CapabilitySet>,
    quotas: QuotaConfig,
    boot: Boot,
    script: &std::path::Path,
) {
    let quotas = Arc::new(QuotaContext::new(quotas));
    metrics::global().bind(sender.clone(), quotas.clone());
    isolate.set_slot(OpContext::new(
        ISOLATE_ID,
        sender,
        permissions.clone(),
        quotas,
    ));
    // Every `Velyr.serve` request lands in the HTTP metrics.
    #[cfg(feature = "web")]
    let _ = set_http_metrics(Arc::new(HttpSink));
    install_globals(
        isolate,
        timers,
        boot,
        worker_host(permissions.clone(), None),
        None,
        permissions,
        Some(script),
    );
    // Last, on both boot paths, so `Velyr` has the same key order with and without a snapshot.
    install_metrics(isolate).expect("metrics bootstrap is valid JS");
}

/// Like [`install`] for a worker isolate, whose op context `WorkerHandle::spawn` already set up
/// with the worker's own authority and quotas.
fn install_worker<I: velyr_engine_api::Isolate>(
    isolate: &mut LoopIsolate<I>,
    timers: TimerHandle,
    boot: Boot,
    host: WorkerHost,
    scope: WorkerScope<'_>,
    permissions: Arc<CapabilitySet>,
) {
    install_globals(isolate, timers, boot, host, Some(scope), permissions, None);
}

/// The same steps with and without a snapshot: the slots, the bootstrap scripts (or the values
/// they left in the snapshot), then what runs per isolate (worker scope, node compat).
fn install_globals<I: velyr_engine_api::Isolate>(
    isolate: &mut LoopIsolate<I>,
    timers: TimerHandle,
    boot: Boot,
    host: WorkerHost,
    scope: Option<WorkerScope<'_>>,
    permissions: Arc<CapabilitySet>,
    // The main isolate's entry, for `process.argv`; workers have none.
    script: Option<&std::path::Path>,
) {
    #[cfg(not(feature = "node"))]
    let _ = script;
    install_slots(isolate, &timers, host, scope.as_ref(), permissions);
    let values = if boot.from_snapshot {
        BootValues::take_from_snapshot(isolate)
    } else {
        BootValues::evaluate(isolate, boot.features)
    }
    .expect("bootstrap scripts are valid JS");
    timers.set_dispatcher(values.timers);
    // `Worker` and the worker global scope; messages need structured clone.
    start_web_api(isolate, &values.messaging, &values.worker, scope)
        .expect("worker bootstrap is valid JS");
    #[cfg(feature = "node")]
    install_node(isolate, timers, script);
    if hardening().frozen_intrinsics {
        freeze_intrinsics(isolate);
    }
}

/// The Rust state the bootstrap scripts' host functions use, installed on every boot.
fn install_slots<I: velyr_engine_api::Isolate>(
    isolate: &mut LoopIsolate<I>,
    timers: &TimerHandle,
    host: WorkerHost,
    scope: Option<&WorkerScope<'_>>,
    permissions: Arc<CapabilitySet>,
) {
    #[cfg(not(feature = "node"))]
    let _ = permissions;
    install_timer_slots(isolate, timers.clone());
    install_blob_slots(isolate);
    install_performance_slots(isolate);
    install_encoding_slots(isolate);
    install_web_streams_slots(isolate);
    install_net_slots(isolate, system_resolver());
    #[cfg(feature = "web")]
    {
        install_tls_slots(isolate);
        install_web_fetch_slots(isolate);
    }
    install_web_api_slots(isolate, Some(host), scope);
    #[cfg(feature = "node")]
    velyr_compat_commonjs::install_commonjs_slots(isolate, permissions);
}

/// Completion values of the bootstrap scripts that the host keeps.
struct BootValues {
    timers: Value,
    messaging: Value,
    worker: Value,
}

impl BootValues {
    /// Runs the profile's bootstrap scripts (no snapshot).
    fn evaluate(
        isolate: &mut impl velyr_engine_api::Isolate,
        features: EngineFeatures,
    ) -> Result<Self, JsException> {
        let mut values = Self {
            timers: Value::Undefined,
            messaging: Value::Undefined,
            worker: Value::Undefined,
        };
        for source in bootstrap::scripts(features) {
            let value = isolate.eval_script(&source)?;
            match source.name.as_str() {
                bootstrap::TIMERS => values.timers = value,
                bootstrap::MESSAGING => values.messaging = value,
                bootstrap::WORKER => values.worker = value,
                _ => {}
            }
        }
        Ok(values)
    }

    /// Reads them from the snapshot's results and deletes those, with the worker messaging ops
    /// `WorkerHandle::spawn` registers after boot (messaging.js deletes them when it runs).
    fn take_from_snapshot(
        isolate: &mut impl velyr_engine_api::Isolate,
    ) -> Result<Self, JsException> {
        tracing::debug!(target: "velyr.cli.snapshot", "isolate booted from the startup snapshot");
        let hidden: Vec<String> = worker_host_functions()
            .iter()
            .map(|def| format!("{:?}", def.name))
            .collect();
        let text = format!(
            "(function (key, ops) {{
              const values = globalThis[key];
              delete globalThis[key];
              for (const op of ops) delete globalThis[op];
              return (name) => values[name];
            }})(Symbol.for({SNAPSHOT_RESULTS_KEY:?}), [{}])",
            hidden.join(", ")
        );
        let source = Source {
            name: "velyr:snapshot".into(),
            text,
        };
        let pick = isolate.eval_script(&source)?;
        let mut take = |name: &str| {
            let name = Value::String(JsString::from(name));
            isolate.call(&pick, &Value::Undefined, &[name])
        };
        Ok(Self {
            timers: take(bootstrap::TIMERS)?,
            messaging: take(bootstrap::MESSAGING)?,
            worker: take(bootstrap::WORKER)?,
        })
    }
}

/// Node compatibility, after `install_commonjs`: `process` is configured first (the module reads
/// it when it loads), then the builtins, then the globals; the `nextTick` drain becomes the
/// loop's tick hook so ticks run before promise callbacks.
#[cfg(feature = "node")]
fn install_node<I: velyr_engine_api::Isolate>(
    isolate: &mut LoopIsolate<I>,
    timers: TimerHandle,
    script: Option<&std::path::Path>,
) {
    use velyr_compat_node as node;
    if let Some(script) = script {
        node::configure_process(isolate, crate::node::process_config(script));
    } else if let Some(on_exit) = velyr_runtime_workers::worker_exit_hook() {
        // A worker's `process.exit(n)` ends that worker with code n, not the whole process.
        let config = node::ProcessConfig {
            on_exit: Some(on_exit),
            ..node::ProcessConfig::default()
        };
        node::configure_process(isolate, config);
    }
    let drain = if hardening().disallow_code_generation {
        // The lazy install boots through JS `eval`, which this flag disables: install eagerly.
        node::install_compat_node(isolate).expect("node bootstrap is valid JS");
        node::install_buffer_global(isolate).expect("buffer global is valid JS");
        node::install_process_global(isolate, timers).expect("process global is valid JS")
    } else {
        // The ~685 KB bootstrap runs on first use of `process`, `Buffer`, a `node:` import or `require`.
        node::install_node_lazy(isolate, timers).expect("node bootstrap is valid JS")
    };
    isolate.set_tick_hook(drain);
}

/// Import resolution for the main isolate and its workers: relative paths as usual, bare
/// specifiers through `node_modules` and `package.json` (`exports`, `imports`). Without the npm
/// resolver (`profile-minimal`, `profile-web`) only relative and absolute specifiers resolve.
// Wiring point for compat-node (VELYR-155): its CommonJS loader joins the node/full profiles here.
pub fn module_resolver() -> Option<ModuleResolverHandle> {
    #[cfg(feature = "npm")]
    {
        use velyr_runtime_modules::StrategyResolver;
        let resolver = velyr_compat_npm::resolve::NpmResolver::new();
        // With the CommonJS transform linked (node, full) packages may resolve to CommonJS files;
        // without it the resolver keeps rejecting them with a clear error. Node compat also matches
        // the `node` condition, as Node does (rxjs hides its working build behind it).
        #[cfg(feature = "node")]
        let resolver = resolver.allow_commonjs().with_node_condition();
        let npm: ModuleResolverHandle = Arc::new(StrategyResolver::new(Arc::new(resolver)));
        #[cfg(feature = "node")]
        let npm = crate::node::resolver(npm);
        Some(npm)
    }
    #[cfg(not(feature = "npm"))]
    None
}

/// Spawner shared by the main isolate and every worker. `policy` is the deployment policy:
/// the CLI has no separate source for one, so it is the main isolate's authority, which a
/// worker's `parent ∩ requested ∩ policy` can never widen.
fn worker_host(policy: Arc<CapabilitySet>, heap_limit_bytes: Option<usize>) -> WorkerHost {
    let spawn_policy = policy.clone();
    WorkerHost {
        deployment_policy: policy,
        heap_limit_bytes,
        spawn: Arc::new(move |parent, request| spawn_worker(parent, request, &spawn_policy)),
    }
}

fn spawn_worker(
    parent: &ParentContext,
    request: SpawnRequest,
    policy: &Arc<CapabilitySet>,
) -> Result<WorkerHandle, VelyrError> {
    let (requested, quotas) = match &request.permissions_json {
        // No `velyr.permissions`: the worker may use whatever its parent can.
        None => (parent.capabilities.clone(), QuotaConfig::default()),
        Some(json) => {
            let flags: Permissions = serde_json::from_str(json).map_err(|err| {
                VelyrError::invalid_data(format!("invalid velyr.permissions: {err}"))
            })?;
            let cwd =
                std::env::current_dir().map_err(|err| VelyrError::internal(err.to_string()))?;
            let built = crate::permissions::build(flags, None, &cwd, &request.entry)
                .map_err(|err| VelyrError::invalid_data(err.to_string()))?;
            (built.capabilities, built.quotas)
        }
    };
    let engine = v8_engine();
    let boot = Boot::of(&engine);
    let (host_functions, bootstrap_js) = boot.bootstrap();
    let options = WorkerOptions {
        entry: request.entry,
        requested: requested.clone(),
        // `velyr.heapLimitMb`, capped at the parent's limit by `WorkerHandle::spawn`.
        heap_limit_bytes: request.heap_limit_bytes,
        config: RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            quotas: Some(quotas),
            leak_check: std::env::var_os("VELYR_LEAK_CHECK").is_some_and(|value| value == "1"),
            leak_collector: Some(take_leak_report),
            module_resolver: crate::transforms::project_resolver(module_resolver(), &requested),
            source_transform: crate::transforms::source_transform(&requested, boot.features),
            ..RuntimeConfig::default()
        },
        host_functions,
        bootstrap_js,
    };
    // Nested workers inherit the heap limit this one runs with.
    let host = worker_host(policy.clone(), parent.heap_limit_bytes);
    let (name, group) = (request.name, request.group);
    WorkerHandle::spawn(engine, parent, options, move |isolate, _sender, timers| {
        let scope = WorkerScope { name: &name, group };
        install_worker(isolate, timers, boot, host, scope, requested);
    })
}

//! What each profile (`profile-minimal|web|node|full`) installs on a fresh isolate.
//!
//! Timers, `URL`, events/abort, text encoding, streams, the file APIs and net are part of every profile, `minimal`
//! included: the kernel needs the first three and they pull in no network code; the file and net APIs stay default-deny.
//! Streams are pure JS plus gzip/deflate ops and need no permissions, so they are not gated either; the same goes for
//! WebCrypto (`crypto`), which only computes over bytes the script already holds.
//! Web crates (fetch, WebSocket, HTTP/TLS) sit behind `cfg(feature = "web")` and the npm resolver behind
//! `cfg(feature = "npm")`; `profile-minimal` links neither (see the features in Cargo.toml).

use std::sync::Arc;
use velyr_engine_api::Isolate as _;
use velyr_engine_api::{Engine, EngineFeatures, HostFunctionDef};
use velyr_engine_v8::V8Engine;
use velyr_io_fs::{fs_host_functions, install_fs};
#[cfg(feature = "web")]
use velyr_io_http::{HttpMetrics, set_http_metrics};
use velyr_io_net::{install_net, net_host_functions};
#[cfg(feature = "web")]
use velyr_io_tls::{install_tls, tls_host_functions};
use velyr_runtime_core::metrics::{self, install_metrics, metrics_host_functions};
use velyr_runtime_core::{LoopIsolate, ModuleResolverHandle};
use velyr_runtime_event_loop::{CompletionSender, TimerHandle};

use velyr_runtime_core::RuntimeConfig;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_ops::{
    BOOTSTRAP_JS, OpContext, OpRegistry, take_leak_report, test_host_functions,
};
use velyr_runtime_permissions::CapabilitySet;
use velyr_runtime_resources::{QuotaConfig, QuotaContext};
use velyr_runtime_workers::{
    ParentContext, SpawnRequest, WorkerHandle, WorkerHost, WorkerOptions, WorkerScope,
    install_web_api, web_api_host_functions,
};
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_performance,
    install_structured_clone, install_timers, install_url, performance_host_functions,
    structured_clone_host_functions, timer_host_functions, url_host_functions,
};
use velyr_web_crypto::{crypto_host_functions, install_web_crypto};
#[cfg(feature = "web")]
use velyr_web_fetch::{fetch_host_functions, install_web_fetch};
use velyr_web_streams::{flate_host_functions, install_web_streams};
#[cfg(feature = "web")]
use velyr_web_websocket::{install_web_websocket, websocket_host_functions};

use crate::permissions::Permissions;

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

pub fn host_functions(features: EngineFeatures) -> Vec<HostFunctionDef> {
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(url_host_functions());
    functions.extend(performance_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(crypto_host_functions());
    functions.extend(fs_host_functions());
    functions.extend(net_host_functions());
    #[cfg(feature = "web")]
    {
        functions.extend(tls_host_functions());
        functions.extend(fetch_host_functions());
        functions.extend(websocket_host_functions());
    }
    functions.extend(metrics_host_functions());
    functions.extend(web_api_host_functions());
    #[cfg(feature = "node")]
    {
        functions.extend(velyr_compat_commonjs::commonjs_host_functions());
        functions.extend(velyr_compat_node::compat_node_host_functions());
    }
    if features.contains(EngineFeatures::SERIALIZE) {
        functions.extend(structured_clone_host_functions());
    }
    functions.extend(test_host_functions());
    functions
}

/// Runs on the main isolate's thread after the op bootstrap and before user code.
pub fn install<I: velyr_engine_api::Isolate>(
    isolate: &mut LoopIsolate<I>,
    sender: CompletionSender,
    timers: TimerHandle,
    permissions: Arc<CapabilitySet>,
    quotas: QuotaConfig,
    features: EngineFeatures,
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
    install_metrics(isolate).expect("metrics bootstrap is valid JS");
    install_globals(
        isolate,
        timers,
        features,
        worker_host(permissions.clone(), None),
        None,
        permissions,
        Some(script),
    );
}

/// Like [`install`] for a worker isolate, whose op context `WorkerHandle::spawn` already set up
/// with the worker's own authority and quotas.
fn install_worker<I: velyr_engine_api::Isolate>(
    isolate: &mut LoopIsolate<I>,
    timers: TimerHandle,
    features: EngineFeatures,
    host: WorkerHost,
    scope: WorkerScope<'_>,
    permissions: Arc<CapabilitySet>,
) {
    install_globals(
        isolate,
        timers,
        features,
        host,
        Some(scope),
        permissions,
        None,
    );
}

fn install_globals<I: velyr_engine_api::Isolate>(
    isolate: &mut LoopIsolate<I>,
    timers: TimerHandle,
    features: EngineFeatures,
    host: WorkerHost,
    scope: Option<WorkerScope<'_>>,
    permissions: Arc<CapabilitySet>,
    // The main isolate's entry, for `process.argv`; workers have none.
    script: Option<&std::path::Path>,
) {
    #[cfg(not(feature = "node"))]
    let _ = (&permissions, script);
    #[cfg(feature = "node")]
    let process_timers = timers.clone();
    install_timers(isolate, timers).expect("timers bootstrap is valid JS");
    install_url(isolate).expect("url bootstrap is valid JS");
    install_performance(isolate).expect("performance bootstrap is valid JS");
    // Before encoding so atob/btoa find DOMException (they look it up at throw time anyway).
    install_events(isolate).expect("events bootstrap is valid JS");
    install_encoding(isolate).expect("encoding bootstrap is valid JS");
    // After events (AbortController) and encoding (TextEncoder/TextDecoder), which it builds on.
    install_web_streams(isolate).expect("streams bootstrap is valid JS");
    // After events (DOMException) and the op context (its ops are async).
    install_web_crypto(isolate).expect("crypto bootstrap is valid JS");
    install_fs(isolate).expect("fs bootstrap is valid JS");
    // After streams: Conn.readable/writable use its native stream hooks.
    install_net(isolate).expect("net bootstrap is valid JS");
    #[cfg(feature = "web")]
    {
        // After net: Velyr.connectTls/listenTls wrap its Conn.
        install_tls(isolate).expect("tls bootstrap is valid JS");
        // After net (resolver), streams (body streams), events (AbortSignal), encoding and URL.
        install_web_fetch(isolate).expect("fetch bootstrap is valid JS");
        // After fetch (Velyr.serve upgrade hook) and Blob.
        install_web_websocket(isolate).expect("websocket bootstrap is valid JS");
    }
    // After events: structured_clone.js rethrows as DOMException when it is available.
    if features.contains(EngineFeatures::SERIALIZE) {
        install_structured_clone(isolate).expect("structured clone bootstrap is valid JS");
    }
    // After events (EventTarget, DOMException) and the op context; messages need structured clone.
    install_web_api(isolate, Some(host), scope).expect("worker bootstrap is valid JS");
    // `require` for CommonJS; `import` of CommonJS goes through the module host's transform.
    #[cfg(feature = "node")]
    {
        velyr_compat_commonjs::install_commonjs(isolate, permissions)
            .expect("commonjs bootstrap is valid JS");
        install_node(isolate, process_timers, script);
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
    }
    node::install_compat_node(isolate).expect("node bootstrap is valid JS");
    node::install_buffer_global(isolate).expect("buffer bootstrap is valid JS");
    let drain =
        node::install_process_global(isolate, timers).expect("process bootstrap is valid JS");
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
        // without it the resolver keeps rejecting them with a clear error.
        #[cfg(feature = "node")]
        let resolver = resolver.allow_commonjs();
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
    let engine = V8Engine::new();
    let features = engine.features();
    let options = WorkerOptions {
        entry: request.entry,
        requested: requested.clone(),
        heap_limit_bytes: None,
        config: RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            quotas: Some(quotas),
            leak_check: std::env::var_os("VELYR_LEAK_CHECK").is_some_and(|value| value == "1"),
            leak_collector: Some(take_leak_report),
            module_resolver: module_resolver(),
            source_transform: crate::transforms::source_transform(&requested),
            ..RuntimeConfig::default()
        },
        host_functions: host_functions(features),
        bootstrap_js: BOOTSTRAP_JS,
    };
    // Nested workers inherit the heap limit this one runs with.
    let host = worker_host(policy.clone(), parent.heap_limit_bytes);
    let (name, group) = (request.name, request.group);
    WorkerHandle::spawn(engine, parent, options, move |isolate, _sender, timers| {
        let scope = WorkerScope { name: &name, group };
        install_worker(isolate, timers, features, host, scope, requested);
    })
}

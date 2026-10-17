//! The host functions and bootstrap scripts this build's profile puts on every isolate, in order.
//! `build.rs` bakes both into the startup snapshot (VELYR-232); without one, `profiles` registers
//! the functions and runs the scripts on each isolate. Compiled into both: no `crate::` paths.

use velyr_engine_api::{EngineFeatures, HostFunctionDef, Source};
use velyr_io_fs::{FS_JS, fs_host_functions};
use velyr_io_net::{NET_JS, net_host_functions};
use velyr_io_process::{COMMAND_JS, process_host_functions};
#[cfg(feature = "web")]
use velyr_io_tls::{TLS_JS, tls_host_functions};
use velyr_runtime_core::metrics::metrics_host_functions;
use velyr_runtime_ops::{OpRegistry, test_host_functions};
use velyr_runtime_workers::{MESSAGING_JS, WORKER_JS, web_api_host_functions};
use velyr_web_core::{
    ABORT_JS, BLOB_JS, DOM_EXCEPTION_JS, ENCODING_JS, EVENT_JS, PERFORMANCE_JS,
    STRUCTURED_CLONE_JS, TIMERS_JS, URL_JS, encoding_host_functions, performance_host_functions,
    structured_clone_host_functions, timer_host_functions, url_host_functions,
};
use velyr_web_crypto::{CRYPTO_JS, crypto_host_functions};
#[cfg(feature = "web")]
use velyr_web_fetch::{FETCH_JS, fetch_host_functions};
use velyr_web_streams::{STREAMS_JS, flate_host_functions};
#[cfg(feature = "web")]
use velyr_web_websocket::{WEBSOCKET_JS, websocket_host_functions};

/// Scripts whose completion value the host keeps: the timer dispatcher, the messaging toolkit
/// and the `Worker` installer.
pub const TIMERS: &str = "velyr:timers";
pub const MESSAGING: &str = "velyr:messaging";
pub const WORKER: &str = "velyr:worker";

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
    functions.extend(process_host_functions());
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

/// The scripts that follow runtime-ops' `<bootstrap>`. None calls a host function while it
/// loads, so the Rust slots they need may be installed later (`profiles::install_slots`).
pub fn scripts(features: EngineFeatures) -> Vec<Source> {
    let mut scripts = vec![
        (TIMERS, TIMERS_JS),
        ("velyr:url", URL_JS),
        ("velyr:blob", BLOB_JS),
        ("velyr:performance", PERFORMANCE_JS),
        // After timers (`AbortSignal.timeout`), before encoding and `structuredClone`, which
        // throw `DOMException`.
        ("velyr:dom-exception", DOM_EXCEPTION_JS),
        ("velyr:event", EVENT_JS),
        ("velyr:abort", ABORT_JS),
        ("velyr:encoding", ENCODING_JS),
        // After events (AbortController) and encoding (TextEncoder/TextDecoder), which it builds on.
        ("velyr:streams", STREAMS_JS),
        ("velyr:crypto", CRYPTO_JS),
        ("velyr:fs", FS_JS),
        // After streams: Conn.readable/writable and ChildProcess.stdout use its native stream hooks.
        ("velyr:net", NET_JS),
        ("velyr:command", COMMAND_JS),
    ];
    // After net (Velyr.connectTls wraps its Conn; fetch resolves through it); WebSocket after
    // fetch (the Velyr.serve upgrade hook).
    #[cfg(feature = "web")]
    scripts.extend([
        ("velyr:tls", TLS_JS),
        ("velyr:fetch", FETCH_JS),
        ("velyr:websocket", WEBSOCKET_JS),
    ]);
    if features.contains(EngineFeatures::SERIALIZE) {
        scripts.push(("velyr:structured-clone", STRUCTURED_CLONE_JS));
    }
    scripts.extend([(MESSAGING, MESSAGING_JS), (WORKER, WORKER_JS)]);
    #[cfg(feature = "node")]
    scripts.push(("velyr:commonjs", velyr_compat_commonjs::COMMONJS_JS));
    scripts
        .into_iter()
        .map(|(name, text)| Source {
            name: name.into(),
            text: text.into(),
        })
        .collect()
}

//! The Velyr runtime a WPT test runs in: the CLI's `profile-full` globals on a fresh V8 isolate.
//!
//! The CLI is a binary crate, so its `profiles` module cannot be reused; this mirrors it. Workers
//! cannot be spawned here (`Worker` throws), and the op context is deny-all.

use std::sync::Arc;

use velyr_engine_api::{EngineFeatures, HostFunctionDef, Isolate};
use velyr_io_fs::{fs_host_functions, install_fs};
use velyr_io_net::{install_net, net_host_functions};
use velyr_io_tls::{install_tls, tls_host_functions};
use velyr_runtime_core::LoopIsolate;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::{CompletionSender, TimerHandle};
use velyr_runtime_ops::{OpRegistry, install_op_context};
use velyr_runtime_permissions::CapabilitySet;
use velyr_runtime_resources::QuotaConfig;
use velyr_runtime_workers::{WorkerHost, install_web_api, web_api_host_functions};
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_performance,
    install_structured_clone, install_timers, install_url, performance_host_functions,
    structured_clone_host_functions, timer_host_functions, url_host_functions,
};
use velyr_web_crypto::{crypto_host_functions, install_web_crypto};
use velyr_web_fetch::{fetch_host_functions, install_web_fetch};
use velyr_web_streams::{flate_host_functions, install_web_streams};

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
    functions.extend(tls_host_functions());
    functions.extend(fetch_host_functions());
    functions.extend(web_api_host_functions());
    if features.contains(EngineFeatures::SERIALIZE) {
        functions.extend(structured_clone_host_functions());
    }
    functions
}

/// Installs the op context and every global, in the order the CLI uses.
pub fn install<I: Isolate>(
    isolate: &mut LoopIsolate<I>,
    sender: CompletionSender,
    timers: TimerHandle,
    features: EngineFeatures,
) {
    let permissions = CapabilitySet::builder().build();
    install_op_context(
        isolate,
        0,
        sender,
        permissions.clone(),
        QuotaConfig::default(),
    );
    install_timers(isolate, timers).expect("timers bootstrap is valid JS");
    install_url(isolate).expect("url bootstrap is valid JS");
    install_performance(isolate).expect("performance bootstrap is valid JS");
    install_events(isolate).expect("events bootstrap is valid JS");
    install_encoding(isolate).expect("encoding bootstrap is valid JS");
    install_web_streams(isolate).expect("streams bootstrap is valid JS");
    install_web_crypto(isolate).expect("crypto bootstrap is valid JS");
    install_fs(isolate).expect("fs bootstrap is valid JS");
    install_net(isolate).expect("net bootstrap is valid JS");
    install_tls(isolate).expect("tls bootstrap is valid JS");
    install_web_fetch(isolate).expect("fetch bootstrap is valid JS");
    if features.contains(EngineFeatures::SERIALIZE) {
        install_structured_clone(isolate).expect("structured clone bootstrap is valid JS");
    }
    let host = WorkerHost {
        deployment_policy: permissions,
        heap_limit_bytes: None,
        spawn: Arc::new(|_, _| {
            Err(VelyrError::unsupported(
                "workers are not available in velyr-wpt",
            ))
        }),
    };
    install_web_api(isolate, Some(host), None).expect("worker bootstrap is valid JS");
}

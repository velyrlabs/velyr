//! Shared harness for the `node:http` / `node:https` tests: a script runs on V8 under the real event
//! loop with io-net installed, over 127.0.0.1 and ephemeral ports only. Every run is bounded by a
//! watchdog, so a regression that hangs the loop fails the test instead of stalling the suite.
//!
//! Each test binary uses a different part of this module.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use velyr_compat_node::{
    ProcessConfig, compat_node_host_functions, configure_process, install_compat_node,
    install_process_global,
};
use velyr_engine_v8::V8Engine;
use velyr_io_net::{install_net, net_host_functions};
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{AddrPattern, Capability, CapabilitySet, HostPattern, PortRange};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, install_url,
    timer_host_functions, url_host_functions,
};
use velyr_web_streams::{flate_host_functions, install_web_streams};

/// How long a script may run before the test fails.
const WATCHDOG: Duration = Duration::from_secs(60);

const ALL_PORTS: PortRange = PortRange {
    start: 0,
    end: 65535,
};

/// What the script may do: connect to / listen on loopback, or nothing.
#[derive(Clone, Copy)]
pub enum Grant {
    Loopback,
    Nothing,
}

const PRELUDE: &str = r#"
const node = globalThis[Symbol.for('velyr.node')];
const req = (name) => node.require(name);
const { Buffer } = req('buffer');
const eq = (a, b, l) => { if (!Object.is(a, b)) throw new Error(`${l || 'eq'}: got ${String(a)}, expected ${String(b)}`); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const once = (emitter, name) => new Promise((resolve, reject) => {
  const onError = (error) => { emitter.off(name, onEvent); reject(error); };
  const onEvent = (...args) => { emitter.off('error', onError); resolve(args); };
  emitter.once(name, onEvent);
  if (name !== 'error') emitter.once('error', onError);
});
const settle = (promise) => promise.then(() => null, (e) => e);
const listening = async (server, host = '127.0.0.1') => {
  server.listen(0, host);
  await once(server, 'listening');
  return server.address().port;
};
// Reads a whole http response (or any Readable) into a Buffer.
const slurp = async (stream) => {
  const parts = [];
  for await (const part of stream) parts.push(part);
  return Buffer.concat(parts);
};
// Sends one request and resolves with { res, body }.
const fetchOnce = (http, options, payload) => new Promise((resolve, reject) => {
  const request = http.request(options, async (res) => resolve({ res, body: (await slurp(res)).toString() }));
  request.on('error', reject);
  request.end(payload);
});
"#;

/// A tiny CommonJS loader for the vendored fixtures: `fixtures` maps a name to its source.
const FIXTURE_LOADER: &str = r#"
const loadedFixtures = {};
const fixture = (name) => {
  if (loadedFixtures[name]) return loadedFixtures[name].exports;
  const module = { exports: {} };
  loadedFixtures[name] = module;
  new Function('module', 'exports', 'require', fixtures[name])(module, module.exports,
    (id) => (id.startsWith('fixture:') ? fixture(id.slice(8)) : req(id)));
  return module.exports;
};
"#;

/// JS defining `fixtures` (name -> source) and `fixture(name)` for the given sources.
pub fn fixture_prelude(sources: &[(&str, &str)]) -> String {
    let map: serde_json::Map<String, serde_json::Value> = sources
        .iter()
        .map(|(name, source)| ((*name).to_owned(), (*source).into()))
        .collect();
    format!(
        "const fixtures = {};\n{FIXTURE_LOADER}",
        serde_json::Value::Object(map)
    )
}

fn run_script(grant: Grant, body: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("main.js");
    let script = format!(
        // A failing script may leave servers open, which keeps the loop alive until the watchdog:
        // print the failure at once so the cause is not lost.
        "(async () => {{\n{PRELUDE}\n{body}\n}})().catch((e) => {{ const m = String((e && e.stack) || e); process.stderr.write('SCRIPT FAILED: ' + m + '\\n'); return Promise.reject(m); }});"
    );
    std::fs::write(&path, script).unwrap();
    let mut builder = CapabilitySet::builder();
    if matches!(grant, Grant::Loopback) {
        for host in ["127.0.0.1", "localhost"] {
            builder = builder.allow(Capability::NetConnect(HostPattern::new(host), ALL_PORTS));
        }
        let listen = AddrPattern::new("127.0.0.1");
        builder = builder.allow(Capability::NetListen(listen, ALL_PORTS));
    }
    let permissions = builder.build();
    let quotas = QuotaConfig::default();
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(url_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(net_host_functions());
    functions.extend(compat_node_host_functions());
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        capabilities: Some(permissions.clone()),
        quotas: Some(quotas.clone()),
        ..RuntimeConfig::default()
    };
    let result = run_file(
        V8Engine::new(),
        config,
        functions,
        BOOTSTRAP_JS,
        move |isolate, sender, timers| {
            install_op_context(isolate, 0, sender, permissions, quotas);
            configure_process(
                isolate,
                ProcessConfig {
                    exec_path: "/opt/velyr/bin/velyr".into(),
                    script: None,
                    args: vec![],
                    on_exit: Some(Arc::new(|_| {})),
                },
            );
            install_timers(isolate, timers.clone()).expect("timers");
            install_events(isolate).expect("events");
            install_url(isolate).expect("url");
            install_encoding(isolate).expect("encoding");
            install_web_streams(isolate).expect("web streams");
            install_net(isolate).expect("net");
            install_compat_node(isolate).expect("compat node");
            let drain = install_process_global(isolate, timers).expect("process global");
            isolate.set_tick_hook(drain);
        },
        path,
    );
    match result {
        Ok(()) => "ok".into(),
        Err(RunError::UnhandledRejection(reason)) => reason,
        Err(RunError::Uncaught(exception)) => exception.message,
        Err(other) => format!("{other:?}"),
    }
}

/// Runs `body` as an async function and returns "ok" or the failure; panics if it takes too long.
pub fn run(grant: Grant, body: &str) -> String {
    let body = body.to_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let finished = runtime.block_on(async {
        tokio::time::timeout(
            WATCHDOG,
            tokio::task::spawn_blocking(move || run_script(grant, &body)),
        )
        .await
    });
    // Dropping the runtime would wait for a stuck script thread, so abandon it.
    runtime.shutdown_background();
    finished
        .expect("the script did not finish before the watchdog")
        .unwrap()
}

pub fn check(grant: Grant, body: &str) {
    assert_eq!(run(grant, body), "ok");
}

pub fn self_signed() -> (String, String) {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    (cert.pem(), signing_key.serialize_pem())
}

//! Shared harness: runs the crypto bootstrap on V8 under the real event loop with an op context (async ops need it).
//! Each test body is an async function; it reports through `record(...)` and any throw becomes a `FAIL` entry.
pub mod fixtures;

use std::path::PathBuf;
use std::sync::Mutex;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_ops::install_op_context;
use velyr_runtime_permissions::CapabilitySet;
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, timer_host_functions,
};
use velyr_web_crypto::{crypto_host_functions, install_web_crypto};

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());

fn record(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("record expects a string".into()));
    };
    LOG.lock().unwrap().push(text.to_string_lossy());
    Ok(Value::Undefined)
}

pub fn run(body: &str) -> Result<Vec<String>, RunError> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let script = format!(
        "(async () => {{ {body} }})().catch((e) => record('FAIL ' + (e && e.stack || e)));"
    );
    let path: PathBuf = std::env::temp_dir().join(format!(
        "velyr-web-crypto-{}-{}.js",
        std::process::id(),
        script.len()
    ));
    std::fs::write(&path, script).unwrap();
    let mut host_functions = timer_host_functions();
    host_functions.extend(encoding_host_functions());
    host_functions.extend(crypto_host_functions());
    host_functions.push(HostFunctionDef {
        name: "record",
        arity: 1,
        callback: record,
    });
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        ..RuntimeConfig::default()
    };
    let result = run_file(
        V8Engine::new(),
        config,
        host_functions,
        "",
        |isolate, sender, timers| {
            let permissions = CapabilitySet::builder().build();
            install_op_context(isolate, 0, sender, permissions, QuotaConfig::default());
            install_timers(isolate, timers).expect("timers bootstrap");
            install_events(isolate).expect("events bootstrap");
            install_encoding(isolate).expect("encoding bootstrap");
            install_web_crypto(isolate).expect("crypto bootstrap");
        },
        path.clone(),
    );
    std::fs::remove_file(&path).ok();
    result.map(|()| LOG.lock().unwrap().clone())
}

/// Runs `body` and asserts it recorded exactly `expected`.
pub fn expect(body: &str, expected: &[&str]) {
    let log = run(body).expect("script runs");
    assert_eq!(log, expected, "body: {body}");
}

/// JS helpers prepended to every vector test: hex and base64url conversion.
#[allow(dead_code)] // each test binary compiles this module on its own and uses a subset
pub const HELPERS: &str = r#"
const hex = (b) => [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, '0')).join('');
const unhex = (h) => new Uint8Array(h.match(/../g).map((x) => parseInt(x, 16)));
const b64u = (bytes) => btoa(String.fromCharCode(...bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
const utf8 = (s) => new TextEncoder().encode(s);
const subtle = crypto.subtle;
"#;

/// Runs `body` with [`HELPERS`] in scope and asserts it recorded exactly `expected`.
#[allow(dead_code)]
pub fn expect_with_helpers(body: &str, expected: &[&str]) {
    expect(&format!("{HELPERS}\n{body}"), expected);
}

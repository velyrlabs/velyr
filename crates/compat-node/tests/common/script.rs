//! Script harness: `run_file` on V8 with the real event loop, the tick hook and an explicit
//! capability set. Scripts are the body of an async function; a failed check throws and comes
//! back as the run result.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use velyr_compat_node::{
    ProcessConfig, compat_node_host_functions, configure_process, emit_before_exit, emit_exit,
    emit_signal, emit_unhandled_rejection, install_compat_node, install_process_global,
};
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Isolate, Source, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{install_op_context, op_async};
use velyr_runtime_permissions::{Capability, CapabilitySet, NamePattern};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, install_url,
    timer_host_functions, url_host_functions,
};
use velyr_web_crypto::{crypto_host_functions, install_web_crypto};
use velyr_web_streams::{flate_host_functions, install_web_streams};

pub static EXITS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Resolves after 20ms from the Tokio runtime: its continuation runs in the loop's I/O phase.
fn op_test_io(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    op_async(ctx, |_| async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(CompletionValue::Undefined)
    })
}

pub fn grants(read: &[&str], write: &[&str]) -> Arc<CapabilitySet> {
    let mut builder = CapabilitySet::builder();
    for name in read {
        builder = builder.allow(Capability::EnvRead(NamePattern::new(*name)));
    }
    for name in write {
        builder = builder.allow(Capability::EnvWrite(NamePattern::new(*name)));
    }
    builder.build()
}

pub fn run_with(body: &str, permissions: Arc<CapabilitySet>, setup: &'static str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("main.js");
    let script = format!(
        "(async () => {{\n{body}\n}})().catch((e) => Promise.reject(String((e && e.stack) || e)));"
    );
    std::fs::write(&path, script).unwrap();
    let mut host_functions = timer_host_functions();
    host_functions.extend(url_host_functions());
    host_functions.extend(encoding_host_functions());
    host_functions.extend(flate_host_functions());
    host_functions.extend(crypto_host_functions());
    host_functions.extend(compat_node_host_functions());
    host_functions.push(HostFunctionDef {
        name: "test_io",
        arity: 0,
        callback: op_test_io,
    });
    let result = run_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            ..RuntimeConfig::default()
        },
        host_functions,
        "",
        move |isolate, sender, timers| {
            install_op_context(isolate, 0, sender, permissions, QuotaConfig::default());
            configure_process(
                isolate,
                ProcessConfig {
                    exec_path: "/opt/velyr/bin/velyr".into(),
                    script: Some("/app/main.js".into()),
                    args: vec!["--flag".into(), "value".into()],
                    on_exit: Some(Arc::new(|code| EXITS.lock().unwrap().push(code))),
                },
            );
            install_timers(isolate, timers.clone()).expect("timers");
            install_events(isolate).expect("events");
            install_url(isolate).expect("url");
            install_encoding(isolate).expect("encoding");
            install_web_streams(isolate).expect("web streams");
            install_web_crypto(isolate).expect("web crypto");
            install_compat_node(isolate).expect("compat node");
            let drain = install_process_global(isolate, timers).expect("process global");
            isolate.set_tick_hook(drain);
            let setup = Source {
                name: "setup".into(),
                text: setup.into(),
            };
            isolate.eval_script(&setup).expect("setup");
            if setup.text.contains("EMIT") {
                emit_before_exit(isolate, 0).unwrap();
                emit_exit(isolate, 7).unwrap();
                assert!(emit_signal(isolate, "SIGINT").unwrap());
                let reason = Value::String("boom".into());
                assert!(emit_unhandled_rejection(isolate, &reason).unwrap());
            }
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

pub fn run(body: &str) -> String {
    run_with(body, grants(&[], &[]), "")
}

pub fn assert_ok(result: String) {
    assert_eq!(result, "ok", "{result}");
}

pub const CHECKS: &str = r#"
const eq = (a, b, l) => { if (!Object.is(a, b)) throw new Error(`${l || 'eq'}: got ${String(a)}, expected ${String(b)}`); };
const same = (a, b, l) => eq(JSON.stringify(a), JSON.stringify(b), l);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
"#;

pub fn check(body: &str) -> String {
    run(&format!("{CHECKS}{body}"))
}

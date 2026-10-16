//! A streamed `Velyr.serve` response body on V8 under the real event loop (VELYR-164): chunks that
//! are ready together leave in one `op_serve_write`; chunks that arrive later still leave one by
//! one, in order.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use velyr_engine_api::{CallContext, HostError, HostFn, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_io_net::{Resolver, install_net_with_resolver, net_host_functions};
use velyr_runtime_core::{RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{AddrPattern, Capability, CapabilitySet, HostPattern, PortRange};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, install_url,
    timer_host_functions, url_host_functions,
};
use velyr_web_fetch::{fetch_host_functions, install_web_fetch};
use velyr_web_streams::{flate_host_functions, install_web_streams};

// `run_file` runs on its own thread, so the probe records into a process-wide log.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());
static WRITES: AtomicUsize = AtomicUsize::new(0);
static REAL_WRITE: OnceLock<HostFn> = OnceLock::new();

/// `op_serve_write` that counts its calls: one call is one hop through the event loop.
fn counting_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    WRITES.fetch_add(1, Ordering::SeqCst);
    REAL_WRITE.get().expect("real op registered")(ctx, args)
}

fn record(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("record expects a string".into()));
    };
    LOG.lock().unwrap().push(text.to_string_lossy());
    Ok(Value::Undefined)
}

fn localhost_resolver() -> Resolver {
    std::sync::Arc::new(|_host, port| {
        Box::pin(async move { Ok(vec![std::net::SocketAddr::from(([127, 0, 0, 1], port))]) })
    })
}

/// The log of the script and how many `op_serve_write` calls it made.
fn run(body: &str) -> (Vec<String>, usize) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    WRITES.store(0, Ordering::SeqCst);
    let script = format!(
        "(async () => {{ {body} }})().catch((e) => record('FAIL ' + (e && e.name) + ' ' + (e && e.message)));"
    );
    let dir = std::env::temp_dir().join(format!(
        "velyr-web-fetch-serve-stream-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("t{}.js", script.len()));
    std::fs::write(&path, script).unwrap();

    let any_port = PortRange {
        start: 0,
        end: 65535,
    };
    let permissions = CapabilitySet::builder()
        .allow(Capability::NetConnect(
            HostPattern::new("127.0.0.1"),
            any_port,
        ))
        .allow(Capability::NetListen(
            AddrPattern::new("127.0.0.1"),
            any_port,
        ))
        .build();
    let quotas = QuotaConfig::default();
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(url_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(net_host_functions());
    for def in fetch_host_functions() {
        if def.name == "op_serve_write" {
            REAL_WRITE.get_or_init(|| def.callback);
            functions.push(HostFunctionDef {
                callback: counting_write,
                ..def
            });
        } else {
            functions.push(def);
        }
    }
    functions.push(HostFunctionDef {
        name: "record",
        arity: 1,
        callback: record,
    });
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        capabilities: Some(permissions.clone()),
        quotas: Some(quotas.clone()),
        leak_check: true,
        ..RuntimeConfig::default()
    };
    let result = run_file(
        V8Engine::new(),
        config,
        functions,
        BOOTSTRAP_JS,
        move |isolate, sender, timers| {
            install_op_context(isolate, 0, sender, permissions, quotas);
            install_timers(isolate, timers).unwrap();
            install_url(isolate).unwrap();
            install_events(isolate).unwrap();
            install_encoding(isolate).unwrap();
            install_web_streams(isolate).unwrap();
            install_net_with_resolver(isolate, localhost_resolver()).unwrap();
            install_web_fetch(isolate).unwrap();
        },
        path,
    );
    result.expect("script runs");
    (LOG.lock().unwrap().clone(), WRITES.load(Ordering::SeqCst))
}

const SERVE: &str = r#"const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, () => new Response(makeBody()));
           try {
             const res = await fetch(`http://127.0.0.1:${server.addr.port}/`);
             const bytes = new Uint8Array(await res.arrayBuffer());
             record(report(bytes));
           } finally { await server.shutdown(); }"#;

fn script(make_body: &str, report: &str) -> String {
    format!(
        "const makeBody = () => {{ {make_body} }}; const report = (bytes) => {{ {report} }}; {SERVE}"
    )
}

#[test]
fn chunks_that_are_ready_together_leave_in_one_write() {
    let (log, writes) = run(&script(
        "let sent = 0; return new ReadableStream({ pull(c) { c.enqueue(new Uint8Array(64).fill(sent)); if (++sent === 16) c.close(); } });",
        "let ok = bytes.length === 1024; for (let i = 0; i < 1024; i++) ok &&= bytes[i] === (i >> 6); return String(ok);",
    ));
    assert_eq!(log, ["true"]);
    assert!(writes <= 2, "16 ready chunks took {writes} writes");
}

#[test]
fn a_long_ready_run_is_batched_and_stays_in_order() {
    let (log, writes) = run(&script(
        "let sent = 0; return new ReadableStream({ pull(c) { c.enqueue(new Uint8Array(1024).fill(sent & 255)); if (++sent === 1000) c.close(); } });",
        "let ok = bytes.length === 1000 * 1024; for (let i = 0; i < bytes.length; i++) ok &&= bytes[i] === ((i >> 10) & 255); return String(ok);",
    ));
    assert_eq!(log, ["true"]);
    assert!(writes < 100, "1000 ready chunks took {writes} writes");
}

#[test]
fn chunks_that_arrive_later_are_not_held_back_or_reordered() {
    let (log, writes) = run(&script(
        "return new ReadableStream({ start(c) { let i = 0; const tick = () => { c.enqueue(new TextEncoder().encode('abc'[i])); if (++i === 3) c.close(); else setTimeout(tick, 5); }; setTimeout(tick, 5); } });",
        "return new TextDecoder().decode(bytes);",
    ));
    assert_eq!(log, ["abc"]);
    assert_eq!(writes, 3, "each chunk is its own flush");
}

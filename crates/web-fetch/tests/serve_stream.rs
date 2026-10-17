//! A streamed `Velyr.serve` response body on V8 under the real event loop (VELYR-164): chunks that
//! are ready together leave in one `op_serve_write`; chunks that arrive later still leave one by
//! one, in order. A string body (VELYR-193) goes to `op_serve_respond` as a string, not encoded in JS.

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
static TAKES: AtomicUsize = AtomicUsize::new(0);
static JSON_TAKES: AtomicUsize = AtomicUsize::new(0);
static REAL_TAKE: OnceLock<HostFn> = OnceLock::new();
static ENCODES: AtomicUsize = AtomicUsize::new(0);
static REAL_ENCODE: OnceLock<HostFn> = OnceLock::new();

/// `op_serve_write` that counts its calls: one call is one hop through the event loop.
fn counting_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    WRITES.fetch_add(1, Ordering::SeqCst);
    REAL_WRITE.get().expect("real op registered")(ctx, args)
}

/// `op_serve_take` that counts its calls and the ones that still answer with a JSON string.
fn counting_take(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    TAKES.fetch_add(1, Ordering::SeqCst);
    let head = REAL_TAKE.get().expect("real op registered")(ctx, args)?;
    if matches!(head, Value::String(_)) {
        JSON_TAKES.fetch_add(1, Ordering::SeqCst);
    }
    Ok(head)
}

/// `op_encoding_encode` that counts its calls (`TextEncoder.encode` in JS).
fn counting_encode(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    ENCODES.fetch_add(1, Ordering::SeqCst);
    REAL_ENCODE.get().expect("real op registered")(ctx, args)
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
    let (log, writes, _) = run_counting(body);
    (log, writes)
}

/// [`run`] plus how many `op_encoding_encode` calls the script made.
fn run_counting(body: &str) -> (Vec<String>, usize, usize) {
    run_with_quotas(body, QuotaConfig::default())
}

fn run_with_quotas(body: &str, quotas: QuotaConfig) -> (Vec<String>, usize, usize) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    WRITES.store(0, Ordering::SeqCst);
    TAKES.store(0, Ordering::SeqCst);
    JSON_TAKES.store(0, Ordering::SeqCst);
    ENCODES.store(0, Ordering::SeqCst);
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
        let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(url_host_functions());
    for def in encoding_host_functions() {
        if def.name == "op_encoding_encode" {
            REAL_ENCODE.get_or_init(|| def.callback);
            functions.push(HostFunctionDef {
                callback: counting_encode,
                ..def
            });
        } else {
            functions.push(def);
        }
    }
    functions.extend(flate_host_functions());
    functions.extend(net_host_functions());
    for def in fetch_host_functions() {
        if def.name == "op_serve_write" {
            REAL_WRITE.get_or_init(|| def.callback);
            functions.push(HostFunctionDef {
                callback: counting_write,
                ..def
            });
        } else if def.name == "op_serve_take" {
            REAL_TAKE.get_or_init(|| def.callback);
            functions.push(HostFunctionDef {
                callback: counting_take,
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
    (
        LOG.lock().unwrap().clone(),
        WRITES.load(Ordering::SeqCst),
        ENCODES.load(Ordering::SeqCst),
    )
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
fn a_string_body_is_sent_as_utf8_without_encoding_in_js() {
    let (log, _, encodes) = run_counting(
        r#"const bodies = {
             "/ascii": () => new Response("x".repeat(100000)),
             "/text": () => new Response("café €😀 a\ud800b"),
             "/empty": () => new Response(""),
             "/json": () => Response.json({ s: "é" }),
           };
           const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, (req) => bodies[new URL(req.url).pathname]());
           try {
             for (const path of Object.keys(bodies)) {
               const res = await fetch(`http://127.0.0.1:${server.addr.port}${path}`);
               const bytes = new Uint8Array(await res.arrayBuffer());
               const shown = path === "/ascii" ? String(bytes.every((b) => b === 0x78)) : Array.from(bytes, (b) => b.toString(16)).join(",");
               record(`${path} ${res.headers.get("content-length")} ${res.headers.get("content-type")} ${bytes.length} ${shown}`);
             }
           } finally { await server.shutdown(); }"#,
    );
    assert_eq!(
        log,
        [
            "/ascii 100000 text/plain;charset=UTF-8 100000 true",
            "/text 19 text/plain;charset=UTF-8 19 63,61,66,c3,a9,20,e2,82,ac,f0,9f,98,80,20,61,ef,bf,bd,62",
            "/empty 0 text/plain;charset=UTF-8 0 ",
            "/json 10 application/json 10 7b,22,73,22,3a,22,c3,a9,22,7d",
        ]
    );
    assert_eq!(encodes, 0, "string bodies were encoded in JS");
}

#[test]
fn a_string_body_still_reads_clones_and_streams_as_utf8() {
    let (log, _) = run(r#"const r = new Response("hé\ud800");
           const c = r.clone();
           record(`${(await r.bytes()).length} ${await c.text()} ${r.bodyUsed} ${c.bodyUsed}`);
           const { value } = await new Response("éb").body.getReader().read();
           record(Array.from(value, (b) => b.toString(16)).join(","));
           const q = new Request("http://127.0.0.1/", { method: "POST", body: "é" });
           record(String((await new Request(q).arrayBuffer()).byteLength));
           record(await new Response(new URLSearchParams("a=é")).text());
           record(Response.json(1, { headers: { "content-type": "x/y" } }).headers.get("content-type"));
           try { Response.json({}, { status: 204 }); record("no throw"); } catch (e) { record(e.name); }
           const j = Response.json({ a: "é" });
           record(`${await j.clone().text()} ${(await j.arrayBuffer()).byteLength}`);"#);
    assert_eq!(
        log,
        [
            "6 h\u{e9}\u{FFFD} true true",
            "c3,a9,62",
            "2",
            "a=%C3%A9",
            "x/y",
            "TypeError",
            "{\"a\":\"\u{e9}\"} 10",
        ]
    );
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

#[test]
fn head_fields_survive_the_array_path() {
    let (log, _) = run(r#"const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, (req, info) => {
             const dup = req.headers.get("x-dup");
             return new Response(`${req.method} ${new URL(req.url).pathname}${new URL(req.url).search} ${dup} ${info.remoteAddr.transport} ${info.remoteAddr.hostname} ${typeof info.remoteAddr.port}`, { headers: [["x-a", "1"], ["x-a", "2"]] });
           });
           try {
             const res = await fetch(`http://127.0.0.1:${server.addr.port}/a%20b?q=%C3%BC`, { method: "PATCH", headers: [["x-dup", "one"], ["x-dup", "two"]], body: "x" });
             record(await res.text());
             record(res.headers.get("x-a"));
           } finally { await server.shutdown(); }"#);
    assert_eq!(log, ["PATCH /a%20b?q=%C3%BC one, two tcp 127.0.0.1 number", "1, 2"]);
    assert_eq!(JSON_TAKES.load(Ordering::SeqCst), 0, "a head still crossed as JSON");
}

#[test]
fn a_burst_is_taken_in_few_ops() {
    let (log, _) = run(r#"const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, (req) => new Response(new URL(req.url).pathname));
           try {
             const all = await Promise.all(Array.from({ length: 50 }, (_, i) =>
               fetch(`http://127.0.0.1:${server.addr.port}/${i}`).then((r) => r.text())));
             record(String(all.every((text, i) => text === `/${i}`)));
           } finally { await server.shutdown(); }"#);
    assert_eq!(log, ["true"]);
    assert!(TAKES.load(Ordering::SeqCst) <= 50);
}

#[test]
fn quota_refused_request_gets_503_and_serving_continues() {
    let quotas = QuotaConfig {
        requests: Some(1),
        ..QuotaConfig::default()
    };
    let (log, _, _) = run_with_quotas(
        r#"let release;
           const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, async (req) => {
             if (new URL(req.url).pathname === "/hold") await new Promise((r) => { release = r; });
             return new Response("ok");
           });
           try {
             const base = `http://127.0.0.1:${server.addr.port}`;
             const held = fetch(`${base}/hold`);
             await new Promise((r) => setTimeout(r, 200));
             record(String((await fetch(`${base}/x`)).status));
             release();
             record(String((await held).status));
             record(String((await fetch(`${base}/x`)).status));
           } finally { await server.shutdown(); }"#,
        quotas,
    );
    assert_eq!(log, ["503", "200", "200"]);
}

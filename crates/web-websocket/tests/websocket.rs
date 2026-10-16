//! `WebSocket` and `Velyr.upgradeWebSocket` on V8 under the real event loop. The server side is
//! `Velyr.serve` in the same isolate.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
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
use velyr_web_websocket::{install_web_websocket, websocket_host_functions};

// `run_file` runs on its own thread, so the probe records into a process-wide log.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());

fn record(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("record expects a string".into()));
    };
    LOG.lock().unwrap().push(text.to_string_lossy());
    Ok(Value::Undefined)
}

fn localhost_resolver() -> Resolver {
    Arc::new(|_host, port| {
        Box::pin(async move { Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]) })
    })
}

/// Runs `body` as an async function; `body_bytes` caps the `body_bytes` quota.
fn run_with(body: &str, body_bytes: Option<u64>) -> Vec<String> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let script = format!(
        "(async () => {{ {body} }})().catch((e) => record('FAIL ' + (e && e.name) + ' ' + (e && e.message)));"
    );
    let dir = std::env::temp_dir().join(format!("velyr-web-websocket-{}", std::process::id()));
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
    let quotas = QuotaConfig {
        body_bytes,
        ..QuotaConfig::default()
    };
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(url_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(net_host_functions());
    functions.extend(fetch_host_functions());
    functions.extend(websocket_host_functions());
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
            install_web_websocket(isolate).unwrap();
        },
        path,
    );
    result.expect("script runs");
    LOG.lock().unwrap().clone()
}

fn run(body: &str) -> Vec<String> {
    run_with(body, None)
}

#[test]
fn echoes_text_and_binary_between_a_client_and_a_velyr_server() {
    let log = run(
        r#"const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, (request) => {
             const { socket, response } = Velyr.upgradeWebSocket(request);
             socket.binaryType = "arraybuffer";
             socket.onmessage = (event) => socket.send(event.data);
             return response;
           });
           try {
             const ws = new WebSocket(`ws://127.0.0.1:${server.addr.port}/chat`);
             ws.binaryType = "arraybuffer";
             record(ws.readyState + " " + ws.url.endsWith("/chat") + " " + ws.binaryType);
             const seen = [];
             await new Promise((resolve, reject) => {
               ws.onopen = () => {
                 record("open " + ws.readyState);
                 ws.send("héllo");
                 ws.send(new Uint8Array([1, 2, 3, 255]).buffer);
                 ws.send(new Blob(["blob!"]));
               };
               ws.onmessage = (event) => {
                 seen.push(typeof event.data === "string" ? event.data : Array.from(new Uint8Array(event.data)).join());
                 if (seen.length === 3) ws.close();
               };
               ws.onerror = () => reject(new Error("socket error"));
               ws.onclose = resolve;
             });
             record(seen.join("|"));
           } finally { await server.shutdown(); }"#,
    );
    assert_eq!(
        log,
        [
            "0 true arraybuffer",
            "open 1",
            "h\u{e9}llo|1,2,3,255|98,108,111,98,33"
        ]
    );
}

#[test]
fn close_code_and_reason_reach_both_sides() {
    let log = run(r#"let serverSide;
           const serverClosed = new Promise((resolve) => { serverSide = resolve; });
           const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, (request) => {
             const { socket, response } = Velyr.upgradeWebSocket(request);
             socket.onclose = (event) => serverSide([event.code, event.reason, event.wasClean, event instanceof Event].join("/"));
             return response;
           });
           try {
             const ws = new WebSocket(`ws://127.0.0.1:${server.addr.port}/`);
             const clientClosed = new Promise((resolve) => { ws.onclose = (event) => resolve([event.code, event.reason, event.wasClean].join("/")); });
             ws.onopen = () => { ws.close(1000, "bye"); record("closing " + ws.readyState); };
             record("client " + await clientClosed + " state " + ws.readyState);
             record("server " + await serverClosed);
           } finally { await server.shutdown(); }"#);
    assert_eq!(
        log,
        [
            "closing 2",
            "client 1000/bye/true state 3",
            "server 1000/bye/true/true"
        ]
    );
}

#[test]
fn message_over_the_body_bytes_limit_closes_with_1009() {
    let log = run_with(
        r#"let serverSide;
           const serverClosed = new Promise((resolve) => { serverSide = resolve; });
           const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, (request) => {
             const { socket, response } = Velyr.upgradeWebSocket(request);
             socket.onmessage = () => record("server got a message");
             socket.onclose = (event) => serverSide(event.code + "/" + event.wasClean);
             return response;
           });
           try {
             const ws = new WebSocket(`ws://127.0.0.1:${server.addr.port}/`);
             const clientClosed = new Promise((resolve) => { ws.onclose = (event) => resolve(event.code); });
             ws.onopen = () => { ws.send("small"); ws.send("x".repeat(4096)); };
             record("client " + await clientClosed);
             record("server " + await serverClosed);
           } finally { await server.shutdown(); }"#,
        Some(1024),
    );
    assert_eq!(
        log,
        ["server got a message", "client 1009", "server 1009/false"]
    );
}

#[test]
fn subprotocol_is_negotiated() {
    let log = run(
        r#"const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, (request) => {
             const { socket, response } = Velyr.upgradeWebSocket(request, { protocol: "b" });
             socket.onopen = () => socket.send("server:" + socket.protocol);
             return response;
           });
           try {
             const ws = new WebSocket(`ws://127.0.0.1:${server.addr.port}/`, ["a", "b"]);
             await new Promise((resolve) => {
               ws.onmessage = (event) => { record(event.data + " client:" + ws.protocol); ws.close(); };
               ws.onclose = resolve;
             });
           } finally { await server.shutdown(); }"#,
    );
    assert_eq!(log, ["server:b client:b"]);
}

#[test]
fn constructor_and_methods_validate_their_input() {
    let log = run(
        r#"const name = (fn) => { try { fn(); return "no error"; } catch (e) { return e.name; } };
           record(name(() => new WebSocket("ws://127.0.0.1:1/#frag")));
           record(name(() => new WebSocket("ftp://127.0.0.1/")));
           record(name(() => new WebSocket("ws://127.0.0.1:1/", ["a", "a"])));
           record(name(() => new WebSocket("ws://127.0.0.1:1/", ["bad protocol"])));
           record(name(() => new WebSocket("not a url")));
           const ws = new WebSocket("ws://127.0.0.1:1/");
           record(name(() => ws.send("early")));
           record(name(() => ws.close(1001)));
           record(name(() => ws.close(1000, "x".repeat(124))));
           record(name(() => { ws.binaryType = "nope"; }));
           record(WebSocket.CONNECTING + "" + WebSocket.OPEN + WebSocket.CLOSING + WebSocket.CLOSED);
           record(Object.prototype.toString.call(ws));
           await new Promise((resolve) => { ws.onclose = resolve; });"#,
    );
    assert_eq!(
        log,
        [
            "SyntaxError",
            "SyntaxError",
            "SyntaxError",
            "SyntaxError",
            "SyntaxError",
            "InvalidStateError",
            "InvalidAccessError",
            "SyntaxError",
            "no error",
            "0123",
            "[object WebSocket]"
        ]
    );
}

#[test]
fn connecting_to_an_ungranted_host_fails_with_error_then_close_1006() {
    let log = run(r#"const ws = new WebSocket("ws://192.0.2.1:9/");
           const events = [];
           await new Promise((resolve) => {
             ws.onerror = () => events.push("error");
             ws.onclose = (event) => { events.push("close " + event.code + " " + event.wasClean); resolve(); };
           });
           record(events.join(",") + " " + ws.readyState);"#);
    assert_eq!(log, ["error,close 1006 false 3"]);
}

#[test]
fn upgrade_requires_a_websocket_request_from_velyr_serve() {
    let log = run(
        r#"const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, (request) => {
             try { Velyr.upgradeWebSocket(request); record("no error"); } catch (e) { record(e.name + " " + e.message.includes("Upgrade")); }
             try { Velyr.upgradeWebSocket(new Request("http://x/")); record("no error"); } catch (e) { record(e.name + " " + e.message.includes("Velyr.serve")); }
             return new Response("plain");
           });
           try { await (await fetch(`http://127.0.0.1:${server.addr.port}/`)).text(); }
           finally { await server.shutdown(); }"#,
    );
    assert_eq!(log, ["TypeError true", "TypeError true"]);
}

//! `Velyr.connect` / `Velyr.listen` on V8 under the real event loop, with capability sets built directly.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_io_net::{Resolver, install_net, install_net_with_resolver, net_host_functions};
use velyr_runtime_core::{RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{AddrPattern, Capability, CapabilitySet, HostPattern, PortRange};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, timer_host_functions,
};
use velyr_web_streams::{flate_host_functions, install_web_streams};

// run_file executes on its own thread, so the probe records into a process-wide log.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());

const ALL_PORTS: PortRange = PortRange {
    start: 0,
    end: 65535,
};

fn record(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("record expects a string".into()));
    };
    LOG.lock().unwrap().push(text.to_string_lossy());
    Ok(Value::Undefined)
}

struct Setup {
    connect: Vec<&'static str>,
    listen: Vec<&'static str>,
    sockets_quota: Option<u64>,
    resolver: Option<Resolver>,
}

impl Setup {
    fn new() -> Self {
        Self {
            connect: vec![],
            listen: vec![],
            sockets_quota: None,
            resolver: None,
        }
    }
}

fn run(setup: Setup, body: &str) -> Vec<String> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let script = format!(
        "(async () => {{ {body} }})().catch((e) => record('FAIL ' + (e && e.stack || e)));"
    );
    let dir = std::env::temp_dir().join(format!("velyr-io-net-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("t{}.js", script.len()));
    std::fs::write(&path, script).unwrap();

    let mut builder = CapabilitySet::builder();
    for host in &setup.connect {
        builder = builder.allow(Capability::NetConnect(HostPattern::new(*host), ALL_PORTS));
    }
    for addr in &setup.listen {
        builder = builder.allow(Capability::NetListen(AddrPattern::new(*addr), ALL_PORTS));
    }
    let permissions = builder.build();
    let quotas = QuotaConfig {
        sockets: setup.sockets_quota,
        ..QuotaConfig::default()
    };
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(net_host_functions());
    functions.push(HostFunctionDef {
        name: "record",
        arity: 1,
        callback: record,
    });
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        capabilities: Some(permissions.clone()),
        quotas: Some(quotas.clone()),
        ..RuntimeConfig::default()
    };
    let resolver = setup.resolver;
    let result = run_file(
        V8Engine::new(),
        config,
        functions,
        BOOTSTRAP_JS,
        move |isolate, sender, timers| {
            install_op_context(isolate, 0, sender, permissions, quotas);
            install_timers(isolate, timers).unwrap();
            install_events(isolate).unwrap();
            install_encoding(isolate).unwrap();
            install_web_streams(isolate).unwrap();
            match resolver {
                Some(resolver) => install_net_with_resolver(isolate, resolver).unwrap(),
                None => install_net(isolate).unwrap(),
            }
        },
        path,
    );
    result.expect("script runs");
    LOG.lock().unwrap().clone()
}

/// Accepts connections forever on a loopback port, answering each with the first bytes it reads.
fn echo_once_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            std::thread::spawn(move || {
                let mut buf = [0u8; 64];
                let n = stream.read(&mut buf).unwrap_or(0);
                let _ = stream.write_all(&buf[..n]);
            });
        }
    });
    port
}

#[test]
fn listen_without_grant_is_permission_denied() {
    let log = run(
        Setup::new(),
        "try { await Velyr.listen({ hostname: '127.0.0.1', port: 0 }); record('listened'); }
         catch (e) { record(e.code); }",
    );
    assert_eq!(log, ["VELYR_PERMISSION_DENIED"]);
}

#[test]
fn connect_without_grant_is_permission_denied() {
    let port = echo_once_server();
    let log = run(
        Setup::new(),
        &format!(
            "try {{ await Velyr.connect({{ hostname: '127.0.0.1', port: {port} }}); record('connected'); }}
             catch (e) {{ record(e.code); }}"
        ),
    );
    assert_eq!(log, ["VELYR_PERMISSION_DENIED"]);
}

#[test]
fn echo_through_streams() {
    let mut setup = Setup::new();
    setup.connect = vec!["127.0.0.1"];
    setup.listen = vec!["127.0.0.1"];
    let log = run(
        setup,
        "const l = await Velyr.listen({ hostname: '127.0.0.1', port: 0 });
         const server = (async () => {
           for await (const c of l) { await c.readable.pipeTo(c.writable); c.close(); break; }
         })();
         const c = await Velyr.connect({ hostname: '127.0.0.1', port: l.addr.port });
         const w = c.writable.getWriter();
         await w.write(new TextEncoder().encode('hello echo'));
         await w.close();
         const bytes = [];
         for await (const chunk of c.readable) bytes.push(...chunk);
         record(new TextDecoder().decode(new Uint8Array(bytes)));
         c.close();
         await server;
         l.close();",
    );
    assert_eq!(log, ["hello echo"]);
}

#[test]
fn read_write_addresses_and_options() {
    let port = echo_once_server();
    let mut setup = Setup::new();
    setup.connect = vec!["127.0.0.1"];
    let log = run(
        setup,
        &format!(
            "const c = await Velyr.connect({{ hostname: '127.0.0.1', port: {port} }});
             c.setNoDelay(true); c.setKeepAlive(true);
             record(c.remoteAddr.hostname + ':' + (c.remoteAddr.port === {port}) + ':' + (c.localAddr.port > 0));
             record(String(await c.write(new TextEncoder().encode('ping'))));
             const buf = new Uint8Array(16);
             const n = await c.read(buf);
             record(n + ':' + new TextDecoder().decode(buf.subarray(0, n)));
             record(String(await c.read(buf)));
             c.close(); c.close();"
        ),
    );
    assert_eq!(log, ["127.0.0.1:true:true", "4", "4:ping", "null"]);
}

#[test]
fn hostname_resolving_to_a_private_address_is_denied() {
    let port = echo_once_server();
    let resolver: Resolver = Arc::new(move |_host, port| {
        Box::pin(async move { Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]) })
    });
    let mut setup = Setup::new();
    setup.connect = vec!["evil.test"];
    setup.resolver = Some(resolver);
    let log = run(
        setup,
        &format!(
            "try {{ await Velyr.connect({{ hostname: 'evil.test', port: {port} }}); record('connected'); }}
             catch (e) {{ record(e.code); }}"
        ),
    );
    assert_eq!(log, ["VELYR_PERMISSION_DENIED"]);
}

#[test]
fn second_connection_exceeds_sockets_quota_until_one_closes() {
    let port = echo_once_server();
    let mut setup = Setup::new();
    setup.connect = vec!["127.0.0.1"];
    setup.sockets_quota = Some(1);
    let log = run(
        setup,
        &format!(
            "const open = () => Velyr.connect({{ hostname: '127.0.0.1', port: {port} }});
             const first = await open();
             try {{ await open(); record('second connected'); }} catch (e) {{ record(e.code); }}
             first.close();
             const third = await open();
             record('third connected');
             third.close();"
        ),
    );
    assert_eq!(log, ["VELYR_QUOTA_EXCEEDED", "third connected"]);
}

#[tokio::test]
async fn system_resolver_resolves_localhost() {
    let resolver = velyr_io_net::system_resolver();
    let addrs = resolver("localhost".to_owned(), 80).await.unwrap();
    assert!(
        addrs
            .iter()
            .all(|addr| addr.ip().is_loopback() && addr.port() == 80)
    );
    assert!(!addrs.is_empty());
}

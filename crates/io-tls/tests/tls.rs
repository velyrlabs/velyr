//! `Velyr.connectTls` / `listenTls` / `startTls` on V8 under the real event loop. The independent
//! peer is a tokio-rustls server (or client) built in the test with a self-signed rcgen certificate.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use rustls::crypto::aws_lc_rs::default_provider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_io_net::{Resolver, install_net_with_resolver, net_host_functions};
use velyr_io_tls::{install_tls, tls_host_functions};
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

struct Cert {
    cert_pem: String,
    key_pem: String,
    cert_der: CertificateDer<'static>,
    key_der: Vec<u8>,
}

fn self_signed() -> Cert {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    Cert {
        cert_pem: cert.pem(),
        key_pem: signing_key.serialize_pem(),
        cert_der: cert.der().clone(),
        key_der: signing_key.serialize_der(),
    }
}

/// Echoes one read on every accepted connection after a TLS handshake offering `alpn`.
fn tls_echo_server(cert: &Cert, alpn: &[&[u8]]) -> u16 {
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.key_der.clone()));
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert_der.clone()], key)
        .unwrap();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap().port()).unwrap();
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let mut buf = [0u8; 64];
                    let n = tls.read(&mut buf).await.unwrap_or(0);
                    let _ = tls.write_all(&buf[..n]).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
    });
    rx.recv().unwrap()
}

struct Setup {
    connect: Vec<&'static str>,
    listen: Vec<&'static str>,
}

fn localhost_resolver() -> Resolver {
    Arc::new(|_host, port| {
        Box::pin(async move { Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]) })
    })
}

fn run(setup: Setup, body: &str) -> Vec<String> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let script = format!(
        "(async () => {{ {body} }})().catch((e) => record('FAIL ' + (e && e.stack || e)));"
    );
    let dir = std::env::temp_dir().join(format!("velyr-io-tls-{}", std::process::id()));
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
    let quotas = QuotaConfig::default();
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(net_host_functions());
    functions.extend(tls_host_functions());
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
            install_net_with_resolver(isolate, localhost_resolver()).unwrap();
            install_tls(isolate).unwrap();
        },
        path,
    );
    result.expect("script runs");
    LOG.lock().unwrap().clone()
}

fn js_str(text: &str) -> String {
    format!("{text:?}")
}

#[test]
fn connect_tls_with_ca_certs_echoes() {
    let cert = self_signed();
    let port = tls_echo_server(&cert, &[]);
    let log = run(
        Setup {
            connect: vec!["localhost"],
            listen: vec![],
        },
        &format!(
            "const c = await Velyr.connectTls({{ hostname: 'localhost', port: {port}, caCerts: [{}] }});
             await c.write(new TextEncoder().encode('secure hello'));
             const buf = new Uint8Array(64);
             const n = await c.read(buf);
             record(new TextDecoder().decode(buf.subarray(0, n)));
             c.close();",
            js_str(&cert.cert_pem)
        ),
    );
    assert_eq!(log, ["secure hello"]);
}

#[test]
fn connect_tls_without_ca_certs_fails_with_tls_error() {
    let cert = self_signed();
    let port = tls_echo_server(&cert, &[]);
    let log = run(
        Setup {
            connect: vec!["localhost"],
            listen: vec![],
        },
        &format!(
            "try {{ await Velyr.connectTls({{ hostname: 'localhost', port: {port} }}); record('connected'); }}
             catch (e) {{ record(e.code); }}"
        ),
    );
    assert_eq!(log, ["VELYR_TLS"]);
}

#[test]
fn connect_tls_without_grant_is_permission_denied() {
    let cert = self_signed();
    let port = tls_echo_server(&cert, &[]);
    let log = run(
        Setup {
            connect: vec![],
            listen: vec![],
        },
        &format!(
            "try {{ await Velyr.connectTls({{ hostname: 'localhost', port: {port} }}); record('connected'); }}
             catch (e) {{ record(e.code); }}"
        ),
    );
    assert_eq!(log, ["VELYR_PERMISSION_DENIED"]);
}

#[test]
fn negotiated_alpn_protocol_is_reported() {
    let cert = self_signed();
    let port = tls_echo_server(&cert, &[b"h2", b"http/1.1"]);
    let log = run(
        Setup {
            connect: vec!["localhost"],
            listen: vec![],
        },
        &format!(
            "const open = (alpnProtocols) => Velyr.connectTls({{ hostname: 'localhost', port: {port},
               caCerts: [{}], alpnProtocols }});
             const h2 = await open(['h2', 'http/1.1']);
             record(String(h2.alpnProtocol));
             h2.close();
             const none = await open();
             record(String(none.alpnProtocol));
             none.close();",
            js_str(&cert.cert_pem)
        ),
    );
    assert_eq!(log, ["h2", "null"]);
}

#[test]
fn start_tls_upgrades_a_plain_connection() {
    let cert = self_signed();
    let port = tls_echo_server(&cert, &[]);
    let log = run(
        Setup {
            connect: vec!["localhost"],
            listen: vec![],
        },
        &format!(
            "const plain = await Velyr.connect({{ hostname: 'localhost', port: {port} }});
             const c = await Velyr.startTls(plain, {{ hostname: 'localhost', caCerts: [{}] }});
             await c.write(new TextEncoder().encode('upgraded'));
             const buf = new Uint8Array(64);
             const n = await c.read(buf);
             record(new TextDecoder().decode(buf.subarray(0, n)));
             c.close();",
            js_str(&cert.cert_pem)
        ),
    );
    assert_eq!(log, ["upgraded"]);
}

#[test]
fn listen_tls_and_connect_tls_echo_through_streams() {
    let cert = self_signed();
    let log = run(
        Setup {
            connect: vec!["localhost"],
            listen: vec!["127.0.0.1"],
        },
        &format!(
            "const cert = {};
             const key = new TextEncoder().encode({});
             const l = await Velyr.listenTls({{ hostname: '127.0.0.1', port: 0, cert, key,
               alpnProtocols: ['h2'] }});
             const server = (async () => {{
               for await (const c of l) {{
                 record('server alpn ' + c.alpnProtocol);
                 await c.readable.pipeTo(c.writable);
                 c.close();
                 break;
               }}
             }})();
             const c = await Velyr.connectTls({{ hostname: 'localhost', port: l.addr.port,
               caCerts: [cert], alpnProtocols: ['h2'] }});
             const w = c.writable.getWriter();
             await w.write(new TextEncoder().encode('tls echo'));
             await w.close();
             const bytes = [];
             for await (const chunk of c.readable) bytes.push(...chunk);
             record(new TextDecoder().decode(new Uint8Array(bytes)));
             c.close();
             await server;
             l.close();",
            js_str(&cert.cert_pem),
            js_str(&cert.key_pem)
        ),
    );
    assert_eq!(log, ["server alpn h2", "tls echo"]);
}

/// Accepts TCP connections and never says a word.
fn silent_server() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            held.push(stream.unwrap());
        }
    });
    port
}

#[test]
fn connect_tls_to_a_silent_peer_times_out() {
    let port = silent_server();
    let log = run(
        Setup {
            connect: vec!["localhost"],
            listen: vec![],
        },
        &format!(
            "const started = Date.now();
             try {{ await Velyr.connectTls({{ hostname: 'localhost', port: {port}, handshakeTimeoutMs: 200 }}); record('connected'); }}
             catch (e) {{ record(e.code); }}
             record(Date.now() - started < 3000 ? 'in time' : 'too slow');"
        ),
    );
    assert_eq!(log, ["VELYR_TIMEOUT", "in time"]);
}

#[test]
fn listen_tls_drops_a_slow_client_without_blocking_accept() {
    let cert = self_signed();
    let log = run(
        Setup {
            connect: vec!["localhost", "127.0.0.1"],
            listen: vec!["127.0.0.1"],
        },
        &format!(
            "const cert = {};
             const key = {};
             const l = await Velyr.listenTls({{ hostname: '127.0.0.1', port: 0, cert, key, handshakeTimeoutMs: 5000 }});
             const server = (async () => {{
               for await (const c of l) {{
                 const buf = new Uint8Array(16);
                 const n = await c.read(buf);
                 await c.write(buf.subarray(0, n));
                 c.close();
                 break;
               }}
             }})();
             const started = Date.now();
             const silent = await Velyr.connect({{ hostname: '127.0.0.1', port: l.addr.port }});
             const c = await Velyr.connectTls({{ hostname: 'localhost', port: l.addr.port, caCerts: [cert] }});
             await c.write(new TextEncoder().encode('fast'));
             const buf = new Uint8Array(16);
             const n = await c.read(buf);
             record(new TextDecoder().decode(buf.subarray(0, n)) + (Date.now() - started < 2000 ? '' : ' (blocked)'));
             c.close();
             await server;
             silent.close();
             l.close();",
            js_str(&cert.cert_pem),
            js_str(&cert.key_pem)
        ),
    );
    assert_eq!(log, ["fast"]);
}

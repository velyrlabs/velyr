//! `node:net`, `node:tls` and `node:dns` on V8 under the real event loop, over 127.0.0.1 and ephemeral
//! ports only. Own harness: io-net and io-tls need the `Velyr` global, which `common` does not install.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use velyr_compat_node::{
    ProcessConfig, compat_node_host_functions, configure_process, install_compat_node,
    install_process_global,
};
use velyr_engine_v8::V8Engine;
use velyr_io_net::{install_net, net_host_functions};
use velyr_io_tls::{install_tls, tls_host_functions};
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{AddrPattern, Capability, CapabilitySet, HostPattern, PortRange};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, install_url,
    timer_host_functions, url_host_functions,
};
use velyr_web_streams::{flate_host_functions, install_web_streams};

// Scripts run on their own thread and one at a time: ports are ephemeral, but the loop is not cheap.
static SERIAL: Mutex<()> = Mutex::new(());

const ALL_PORTS: PortRange = PortRange {
    start: 0,
    end: 65535,
};

/// What the script may do: connect to / listen on loopback (with a sockets quota, if given), or nothing.
enum Grant {
    Loopback,
    LoopbackSockets(u64),
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
"#;

fn run(grant: Grant, body: &str) -> String {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("main.js");
    let script = format!(
        "(async () => {{\n{PRELUDE}\n{body}\n}})().catch((e) => Promise.reject(String((e && e.stack) || e)));"
    );
    std::fs::write(&path, script).unwrap();
    let mut builder = CapabilitySet::builder();
    if !matches!(grant, Grant::Nothing) {
        for host in ["127.0.0.1", "localhost"] {
            builder = builder.allow(Capability::NetConnect(HostPattern::new(host), ALL_PORTS));
        }
        let listen = AddrPattern::new("127.0.0.1");
        builder = builder.allow(Capability::NetListen(listen, ALL_PORTS));
    }
    let permissions = builder.build();
    let quotas = QuotaConfig {
        sockets: match grant {
            Grant::LoopbackSockets(limit) => Some(limit),
            _ => None,
        },
        ..QuotaConfig::default()
    };
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(url_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(net_host_functions());
    functions.extend(tls_host_functions());
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
            install_tls(isolate).expect("tls");
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

fn check(grant: Grant, body: &str) {
    assert_eq!(run(grant, body), "ok");
}

fn self_signed() -> (String, String) {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    (cert.pem(), signing_key.serialize_pem())
}

#[test]
fn echo_server_and_client() {
    check(
        Grant::Loopback,
        r#"
        const net = req('net');
        const server = net.createServer((socket) => socket.pipe(socket));
        const port = await listening(server);
        const client = net.connect({ port, host: '127.0.0.1' });
        const chunks = [];
        client.on('data', (chunk) => chunks.push(Buffer.from(chunk)));
        await once(client, 'connect');
        eq(`${client.remoteAddress}:${client.remotePort} ${client.remoteFamily}`, `127.0.0.1:${port} IPv4`);
        // The socket hands io-net's chunks over as Buffer views; that is only free if they alias.
        const bytes = new ArrayBuffer(8);
        Buffer.from(bytes, 2, 4)[0] = 9;
        eq(new Uint8Array(bytes)[2], 9, 'Buffer.from(arrayBuffer) aliases');
        client.write('hello ');
        client.end('world');
        await once(client, 'close');
        eq(Buffer.concat(chunks).toString(), 'hello world');
        eq(`${client.bytesWritten} ${client.bytesRead}`, '11 11');
        server.close();
        await once(server, 'close');
        "#,
    );
}

#[test]
fn writes_before_connect_are_queued_and_connection_events_fire() {
    check(
        Grant::Loopback,
        r#"
        const net = req('net');
        let seen = '';
        const server = net.createServer((socket) => {
          socket.setEncoding('utf8');
          socket.on('data', (text) => { seen += text; });
          socket.on('end', () => socket.end());
        });
        const port = await listening(server);
        const client = new net.Socket();
        client.write('early');
        eq(client.connecting, false);
        client.connect(port, '127.0.0.1');
        eq(client.connecting, true);
        client.resume();
        client.end();
        await once(client, 'close');
        eq(seen, 'early');
        eq(server.listening, true);
        server.close();
        await once(server, 'close');
        eq(server.listening, false);
        "#,
    );
}

#[test]
fn a_paused_peer_stops_the_writer_and_no_byte_is_lost() {
    check(
        Grant::Loopback,
        r#"
        const net = req('net');
        const MiB = 1024 * 1024;
        const TOTAL = 24;
        let received = 0;
        let held;
        const server = net.createServer((socket) => {
          held = socket;
          socket.pause();
          socket.on('data', (chunk) => { received += chunk.length; });
        });
        const port = await listening(server);
        const client = net.connect(port, '127.0.0.1');
        await once(client, 'connect');
        const chunk = Buffer.alloc(MiB, 7);
        let completed = 0, drains = 0;
        const writer = (async () => {
          for (let i = 0; i < TOTAL; i++) {
            if (!client.write(chunk, () => { completed += 1; })) {
              drains += 1;
              await once(client, 'drain');
            }
          }
        })();
        await sleep(400);
        // Nothing reads the peer, so the kernel buffers fill and the writer parks instead of queueing it all.
        if (completed >= TOTAL) throw new Error(`writer never stalled: ${completed} chunks completed`);
        eq(received, 0, 'paused peer received');
        if (client.writableLength > 4 * MiB) throw new Error(`write buffer grew to ${client.writableLength}`);
        held.resume();
        await writer;
        // Nobody consumes the client's readable side; resume it so the close can complete.
        client.resume();
        client.end();
        await once(client, 'close');
        eq(received, TOTAL * MiB, 'bytes received');
        if (drains === 0) throw new Error('write() never reported backpressure');
        server.close();
        "#,
    );
}

#[test]
fn a_destroyed_socket_surfaces_no_error_from_its_pending_read() {
    check(
        Grant::Loopback,
        r#"
        const net = req('net');
        let theirs;
        const server = net.createServer((socket) => { theirs = socket; socket.on('error', () => {}); });
        const port = await listening(server);
        const client = net.connect(port, '127.0.0.1');
        let errors = 0;
        client.on('error', () => { errors += 1; });
        await once(client, 'connect');
        client.resume();
        await sleep(50);
        client.destroy();
        await once(client, 'close');
        await sleep(50);
        eq(errors, 0);
        eq(client.destroyed, true);
        theirs.destroy();
        server.close();
        await once(server, 'close');
        "#,
    );
}

#[test]
fn listen_and_connect_without_permission_fail_with_err_access_denied() {
    check(
        Grant::Nothing,
        r#"
        const net = req('net');
        const server = net.createServer();
        const failing = once(server, 'error');
        server.listen(0, '127.0.0.1');
        const [listenError] = await failing;
        eq(listenError.code, 'ERR_ACCESS_DENIED', 'listen');
        eq(server.listening, false);
        const client = net.connect({ port: 9, host: '127.0.0.1' });
        const [connectError] = await once(client, 'error');
        eq(connectError.code, 'ERR_ACCESS_DENIED', 'connect');
        eq(client.destroyed, true);
        "#,
    );
}

#[test]
fn a_refused_connection_reports_econnrefused() {
    check(
        Grant::Loopback,
        r#"
        const net = req('net');
        const probe = net.createServer();
        const port = await listening(probe);
        probe.close();
        await once(probe, 'close');
        const client = net.connect(port, '127.0.0.1');
        const [error] = await once(client, 'error');
        eq(error.code, 'ECONNREFUSED');
        "#,
    );
}

#[test]
fn pipes_and_ipc_are_unsupported() {
    check(
        Grant::Loopback,
        r#"
        const net = req('net');
        const code = (fn) => { try { fn(); } catch (e) { return e.code; } return 'none'; };
        const attempts = [() => net.connect({ path: '/tmp/velyr.sock' }), () => net.connect('/tmp/velyr.sock'),
          () => new net.Socket({ fd: 1 }), () => net.createServer().listen('/tmp/velyr.sock'),
          () => net.createServer().listen({ path: '/tmp/velyr.sock' })];
        attempts.forEach((attempt, index) => eq(code(attempt), 'ERR_VELYR_UNSUPPORTED', `attempt ${index}`));
        "#,
    );
}

#[test]
fn a_connection_dropped_over_the_sockets_quota_does_not_stop_the_server() {
    check(
        Grant::LoopbackSockets(3),
        r#"
        const net = req('net');
        const accepted = [];
        const errors = [];
        const server = net.createServer((socket) => { accepted.push(socket); socket.on('error', () => {}); socket.resume(); });
        server.on('error', (error) => errors.push(error));
        const port = await listening(server);
        const connect = async () => {
          const socket = net.connect(port, '127.0.0.1');
          socket.on('error', () => {});
          socket.resume();
          await once(socket, 'connect');
          await sleep(100);
          return socket;
        };
        const first = await connect(); // its own socket plus the accepted one: 2 of 3
        eq(accepted.length, 1);
        const second = await connect(); // its own socket takes the 3rd; the accepted half is dropped
        eq(accepted.length, 1);
        eq(errors.length, 1);
        eq(server.listening, true);
        for (const socket of [second, first, accepted[0]]) socket.destroy();
        await sleep(100);
        const third = await connect();
        eq(accepted.length, 2, 'server kept accepting');
        for (const socket of [third, accepted[1]]) socket.destroy();
        server.close();
        "#,
    );
}

#[test]
fn ip_helpers_and_block_list() {
    check(
        Grant::Nothing,
        r#"
        const net = req('net');
        for (const [text, kind] of [['127.0.0.1', 4], ['::1', 6], ['fe80::1%eth0', 6], ['::ffff:1.2.3.4', 6],
          ['1.2.3', 0], ['256.1.1.1', 0], ['01.2.3.4', 0], [':::1', 0], [42, 0]]) eq(net.isIP(text), kind, String(text));
        eq(net.isIPv4('10.0.0.1') && !net.isIPv4('::1') && net.isIPv6('2001:db8::8a2e:370:7334') && !net.isIPv6('1.1.1.1'), true);
        const list = new net.BlockList();
        list.addAddress('10.0.0.5');
        list.addRange('192.168.0.10', '192.168.0.20');
        list.addSubnet('172.16.0.0', 12);
        list.addSubnet('2001:db8::', 32, 'ipv6');
        for (const [address, type, hit] of [['10.0.0.5', 'ipv4', true], ['10.0.0.6', 'ipv4', false],
          ['192.168.0.15', 'ipv4', true], ['192.168.0.21', 'ipv4', false], ['172.31.255.255', 'ipv4', true],
          ['172.32.0.0', 'ipv4', false], ['2001:db8:1::1', 'ipv6', true], ['2001:db9::1', 'ipv6', false],
          ['::ffff:10.0.0.5', 'ipv6', true]]) eq(list.check(address, type), hit, address);
        eq(list.rules.length, 4);
        eq(net.BlockList.isBlockList(list), true);
        "#,
    );
}

#[test]
fn dns_lookup_resolves_localhost_without_any_permission() {
    check(
        Grant::Nothing,
        r#"
        const dns = req('dns');
        const [address, family] = await new Promise((resolve, reject) =>
          dns.lookup('localhost', (e, a, f) => (e ? reject(e) : resolve([a, f]))));
        if (address !== '127.0.0.1' && address !== '::1') throw new Error(`address ${address}`);
        eq(family, address === '::1' ? 6 : 4);
        const all = await req('dns/promises').lookup('localhost', { all: true });
        if (!Array.isArray(all) || all.length === 0) throw new Error('all');
        const v4 = await dns.promises.lookup('localhost', { family: 4 });
        eq(`${v4.address} ${v4.family} ${(await dns.promises.lookup('10.1.2.3')).address}`, '127.0.0.1 4 10.1.2.3');
        const missing = await settle(dns.promises.lookup('nonexistent.invalid'));
        eq(`${missing.code} ${missing.syscall} ${missing.hostname}`, 'ENOTFOUND getaddrinfo nonexistent.invalid');
        // A lookup grants nothing: connecting to the answer is still denied.
        const net = req('net');
        const client = net.connect({ port: 9, host: 'localhost' });
        const [denied] = await once(client, 'error');
        eq(denied.code, 'ERR_ACCESS_DENIED');
        "#,
    );
}

#[test]
fn dns_result_order_and_argument_checks() {
    check(
        Grant::Nothing,
        r#"
        const dns = req('dns');
        eq(dns.getDefaultResultOrder(), 'verbatim');
        dns.setDefaultResultOrder('ipv4first');
        eq(dns.getDefaultResultOrder(), 'ipv4first');
        dns.setDefaultResultOrder('verbatim');
        eq((await settle(dns.promises.lookup(42))).code, 'ERR_INVALID_ARG_TYPE');
        eq((await settle(dns.promises.lookup('localhost', { family: 5 }))).code, 'ERR_INVALID_ARG_VALUE');
        "#,
    );
}

#[test]
fn tls_server_and_client_echo_with_the_test_certificate() {
    let (cert, key) = self_signed();
    let body = format!(
        r#"
        const tls = req('tls');
        const cert = {cert:?};
        const key = {key:?};
        const server = tls.createServer({{ cert, key, ALPNProtocols: ['velyr'] }}, (socket) => {{
          eq(socket.encrypted, true);
          socket.pipe(socket);
        }});
        const port = await listening(server);
        const client = tls.connect({{ host: '127.0.0.1', port, servername: 'localhost', ca: cert, ALPNProtocols: ['velyr'] }});
        const chunks = [];
        client.on('data', (chunk) => chunks.push(Buffer.from(chunk)));
        await once(client, 'secureConnect');
        eq(client.authorized, true);
        eq(client.encrypted, true);
        eq(client.alpnProtocol, 'velyr');
        client.end('secret');
        await once(client, 'close');
        eq(Buffer.concat(chunks).toString(), 'secret');
        server.close();
        await once(server, 'close');
        "#
    );
    check(Grant::Loopback, &body);
}

#[test]
fn tls_client_rejects_an_untrusted_certificate_and_refuses_to_skip_verification() {
    let (cert, key) = self_signed();
    let body = format!(
        r#"
        const tls = req('tls');
        const cert = {cert:?};
        const key = {key:?};
        const server = tls.createServer({{ cert, key }}, (socket) => socket.end());
        server.on('tlsClientError', () => {{}});
        const port = await listening(server);
        const client = tls.connect({{ host: '127.0.0.1', port, servername: 'localhost' }});
        const [error] = await once(client, 'error');
        eq(error.code, 'VELYR_TLS');
        let code;
        try {{ tls.connect({{ host: '127.0.0.1', port, rejectUnauthorized: false }}); }} catch (e) {{ code = e.code; }}
        eq(code, 'ERR_VELYR_UNSUPPORTED');
        server.close();
        await once(server, 'close');
        "#
    );
    check(Grant::Loopback, &body);
}

#[test]
fn tls_helpers() {
    let (cert, key) = self_signed();
    let body = format!(
        r#"
        const tls = req('tls');
        const ciphers = tls.getCiphers();
        if (!ciphers.includes('tls_aes_256_gcm_sha384')) throw new Error('ciphers');
        const context = tls.createSecureContext({{ cert: {cert:?}, key: {key:?} }});
        eq(context instanceof tls.SecureContext, true);
        const peer = {{ subject: {{ CN: 'ignored.example' }}, subjectaltname: 'DNS:example.com, DNS:*.example.org, IP Address:10.0.0.1' }};
        for (const host of ['example.com', 'a.example.org', '10.0.0.1']) eq(tls.checkServerIdentity(host, peer), undefined, host);
        for (const host of ['a.b.example.org', 'other.com', 'ignored.example']) eq(tls.checkServerIdentity(host, peer).code, 'ERR_TLS_CERT_ALTNAME_INVALID', host);
        eq(tls.checkServerIdentity('cn.example', {{ subject: {{ CN: 'cn.example' }} }}), undefined);
        eq(tls.DEFAULT_MIN_VERSION, 'TLSv1.2');
        "#
    );
    check(Grant::Nothing, &body);
}

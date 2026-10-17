//! `dns.resolve*()` / `dns.reverse()` against a fake DNS server on 127.0.0.1 (a UDP socket on an
//! ephemeral port, reached through `setServers`), so nothing leaves the machine.

use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::Arc;

use velyr_compat_node::{
    ProcessConfig, compat_node_host_functions, configure_process, install_compat_node,
    install_process_global,
};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_ops::install_op_context;
use velyr_runtime_permissions::{Capability, CapabilitySet, HostPattern, PortRange};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{install_timers, timer_host_functions};

const PRELUDE: &str = r#"
const node = globalThis[Symbol.for('velyr.node')];
const req = (name) => node.require(name);
const eq = (a, b, l) => { if (!Object.is(a, b)) throw new Error(`${l || 'eq'}: got ${String(a)}, expected ${String(b)}`); };
const same = (a, b, l) => eq(JSON.stringify(a), JSON.stringify(b), l);
const settle = (promise) => promise.then(() => null, (e) => e);
"#;

fn name_bytes(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.split('.').filter(|label| !label.is_empty()) {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}

/// The rdata the fake server answers for `qtype`, if it knows the type.
fn rdata(qname: &str, qtype: u16) -> Option<Vec<u8>> {
    let target = |name: &str| name_bytes(name);
    Some(match qtype {
        1 => vec![192, 0, 2, 7],
        28 => "2001:db8::7"
            .parse::<std::net::Ipv6Addr>()
            .unwrap()
            .octets()
            .to_vec(),
        2 => target("ns1.example.test"),
        5 => target("alias.example.test"),
        12 if qname.starts_with("3.2.1.10.in-addr") => target("host.example.test"),
        15 => [&[0, 10][..], &target("mail.example.test")].concat(),
        16 => [&[11][..], b"v=spf1 -all", &[6], b"second"].concat(),
        33 => [&[0, 1, 0, 2, 0x1f, 0x90][..], &target("srv.example.test")].concat(),
        6 => {
            let mut soa = [target("ns1.example.test"), target("admin.example.test")].concat();
            for number in [7u32, 3600, 600, 86400, 60] {
                soa.extend_from_slice(&number.to_be_bytes());
            }
            soa
        }
        _ => return None,
    })
}

fn answer(query: &[u8]) -> Vec<u8> {
    let mut at = 12;
    let mut labels = Vec::new();
    while query[at] != 0 {
        let len = query[at] as usize;
        labels.push(String::from_utf8_lossy(&query[at + 1..at + 1 + len]).into_owned());
        at += len + 1;
    }
    let qname = labels.join(".");
    let qtype = u16::from_be_bytes([query[at + 1], query[at + 2]]);
    let end = at + 5;
    let data = if qname.starts_with("missing") {
        None
    } else {
        rdata(&qname, qtype)
    };
    let rcode = if qname.starts_with("missing") { 3 } else { 0 };
    let mut reply = query[..2].to_vec();
    reply.extend_from_slice(&[
        0x81,
        0x80 | rcode,
        0,
        1,
        0,
        u8::from(data.is_some()),
        0,
        0,
        0,
        0,
    ]);
    reply.extend_from_slice(&query[12..end]);
    if let Some(data) = data {
        reply.extend_from_slice(&[0xc0, 0x0c, (qtype >> 8) as u8, qtype as u8, 0, 1]);
        reply.extend_from_slice(&60u32.to_be_bytes());
        reply.extend_from_slice(&(data.len() as u16).to_be_bytes());
        reply.extend_from_slice(&data);
    }
    reply
}

fn fake_dns() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        while let Ok((len, from)) = socket.recv_from(&mut buf) {
            let _ = socket.send_to(&answer(&buf[..len]), from);
        }
    });
    port
}

fn run(allow_loopback: bool, body: &str) -> String {
    run_granting(if allow_loopback { &["127.0.0.1"] } else { &[] }, body)
}

fn run_granting(hosts: &[&str], body: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("main.js");
    let script = format!(
        "(async () => {{\n{PRELUDE}\n{body}\n}})().catch((e) => Promise.reject(String((e && e.stack) || e)));"
    );
    std::fs::write(&path, script).unwrap();
    let mut builder = CapabilitySet::builder();
    let all = PortRange {
        start: 0,
        end: 65535,
    };
    for host in hosts {
        builder = builder.allow(Capability::NetConnect(HostPattern::new(*host), all));
    }
    let permissions = builder.build();
    let mut functions = timer_host_functions();
    functions.extend(compat_node_host_functions());
    let result = run_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            ..RuntimeConfig::default()
        },
        functions,
        "",
        move |isolate, sender, timers| {
            install_op_context(isolate, 0, sender, permissions, QuotaConfig::default());
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

#[test]
fn resolve_and_reverse_answer_from_the_configured_server() {
    let port = fake_dns();
    let body = format!(
        r#"
        const dns = req('dns');
        const p = dns.promises;
        dns.setServers(['127.0.0.1:{port}']);
        same(p.getServers(), ['127.0.0.1:{port}']);
        same(await p.resolve4('a.example.test'), ['192.0.2.7']);
        same(await p.resolve4('a.example.test', {{ ttl: true }}).then((r) => r.map((e) => [e.address, typeof e.ttl])), [['192.0.2.7', 'number']]);
        same(await p.resolve6('a.example.test'), ['2001:db8::7']);
        same(await p.resolve('a.example.test', 'A'), ['192.0.2.7']);
        same(await p.resolveMx('a.example.test'), [{{ priority: 10, exchange: 'mail.example.test' }}]);
        same(await p.resolveTxt('a.example.test'), [['v=spf1 -all', 'second']]);
        same(await p.resolveNs('a.example.test'), ['ns1.example.test']);
        same(await p.resolveCname('a.example.test'), ['alias.example.test']);
        same(await p.resolveSrv('a.example.test'), [{{ priority: 1, weight: 2, port: 8080, name: 'srv.example.test' }}]);
        same(await p.resolveSoa('a.example.test'), {{ nsname: 'ns1.example.test', hostmaster: 'admin.example.test', serial: 7, refresh: 3600, retry: 600, expire: 86400, minttl: 60 }});
        same(await p.reverse('10.1.2.3'), ['host.example.test']);
        const missing = await settle(p.resolve4('missing.example.test'));
        eq(missing.code, 'ENOTFOUND');
        eq(missing.syscall, 'queryA');
        eq(missing.hostname, 'missing.example.test');
        eq((await settle(p.reverse('not an ip'))).code, 'ERR_INVALID_ARG_VALUE');
        eq((await settle(p.resolve('a.example.test', 'BOGUS'))).code, 'ERR_INVALID_ARG_VALUE');
        const viaCallback = await new Promise((resolve, reject) => dns.resolve4('a.example.test', (e, a) => (e ? reject(e) : resolve(a))));
        same(viaCallback, ['192.0.2.7']);
        const fresh = new dns.Resolver();
        same(fresh.getServers(), []);
        fresh.setServers(['[::1]:5353', '10.0.0.1']);
        same(fresh.getServers(), ['[::1]:5353', '10.0.0.1']);
        same(p.getServers(), ['127.0.0.1:{port}']);
        "#
    );
    let result = run(true, &body);
    assert_eq!(result, "ok", "{result}");
}

#[test]
fn a_custom_server_needs_net_connect_permission() {
    let port = fake_dns();
    let body = format!(
        r#"
        const p = req('dns').promises;
        p.setServers(['127.0.0.1:{port}']);
        const denied = await settle(p.resolve4('a.example.test'));
        eq(denied.code, 'ERR_ACCESS_DENIED');
        "#
    );
    let result = run(false, &body);
    assert_eq!(result, "ok", "{result}");
}

#[test]
fn system_resolver_queries_need_net_connect_but_lookup_does_not() {
    let body = r#"
        const dns = req('dns');
        const p = dns.promises;
        eq((await settle(p.resolveTxt('x.example'))).code, 'ERR_ACCESS_DENIED');
        eq((await settle(p.resolve4('x.example'))).code, 'ERR_ACCESS_DENIED');
        eq((await settle(p.reverse('10.1.2.3'))).code, 'ERR_ACCESS_DENIED');
        const viaCallback = await new Promise((resolve) => dns.resolveTxt('x.example', (e) => resolve(e)));
        eq(viaCallback.code, 'ERR_ACCESS_DENIED');
        const found = await p.lookup('localhost');
        eq(typeof found.address, 'string');
        "#;
    let result = run_granting(&[], body);
    assert_eq!(result, "ok", "{result}");
}

#[test]
fn system_resolver_queries_are_scoped_to_the_queried_host() {
    let body = r#"
        const p = req('dns').promises;
        eq((await settle(p.resolveTxt('x.example'))).code, 'ERR_ACCESS_DENIED');
        "#;
    let result = run_granting(&["other.example"], body);
    assert_eq!(result, "ok", "{result}");
}

#[test]
fn system_resolver_queries_pass_authorization_with_net_connect() {
    // The network may be absent, so only the authorization is asserted: any failure but a denial.
    let body = r#"
        const p = req('dns').promises;
        const error = await settle(p.resolveTxt('x.example'));
        if (error && error.code === 'ERR_ACCESS_DENIED') throw new Error('denied: ' + error.message);
        const reversed = await settle(p.reverse('10.1.2.3'));
        if (reversed && reversed.code === 'ERR_ACCESS_DENIED') throw new Error('denied: ' + reversed.message);
        "#;
    let result = run_granting(&["x.example", "3.2.1.10.in-addr.arpa"], body);
    assert_eq!(result, "ok", "{result}");
}

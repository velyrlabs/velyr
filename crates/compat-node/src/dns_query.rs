//! Host op behind `dns.resolve*()` and `dns.reverse()`: `op_node_dns_query(name, rrtype, servers)`
//! sends one DNS query with hickory-resolver and answers a JSON array of
//! `{ "type", "ttl", ...fields }`, one entry per answer record of the asked type.
//!
//! `servers` is empty (use the system configuration: `/etc/resolv.conf`) or one `ip port` pair per
//! line (`Resolver#setServers`). A custom server is a network destination, so each one needs
//! `net-connect` for its address and port. The system servers need `net-connect` for the queried
//! name on port 53 (see `authorize_system_query`); only `dns.lookup` stays permission-free.
//! Failures read `CODE|detail` with a Node code (`ENOTFOUND`, `ENODATA`, `ETIMEOUT`, `ESERVFAIL`).
//! A resolver is built per call, so nothing is cached between queries.

use std::net::IpAddr;

use hickory_resolver::TokioResolver;
use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolverConfig};
use hickory_resolver::net::NetError;
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::{RData, RecordType};
use serde_json::{Map, Value as Json, json};
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{authorize, op_async};
use velyr_runtime_permissions::Request;

const SYSTEM_DNS_PORT: u16 = 53;

pub(crate) fn dns_query_host_functions() -> Vec<HostFunctionDef> {
    vec![HostFunctionDef {
        name: "op_node_dns_query",
        arity: 3,
        callback: op_dns_query,
    }]
}

fn string_arg(args: &[Value], index: usize) -> Result<String, HostError> {
    match args.get(index) {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        _ => Err(HostError::TypeError(format!(
            "op_node_dns_query expects argument {index} to be a string"
        ))),
    }
}

fn record_type(name: &str) -> Option<RecordType> {
    Some(match name {
        "A" => RecordType::A,
        "AAAA" => RecordType::AAAA,
        "ANY" => RecordType::ANY,
        "CAA" => RecordType::CAA,
        "CNAME" => RecordType::CNAME,
        "MX" => RecordType::MX,
        "NAPTR" => RecordType::NAPTR,
        "NS" => RecordType::NS,
        "PTR" => RecordType::PTR,
        "SOA" => RecordType::SOA,
        "SRV" => RecordType::SRV,
        "TXT" => RecordType::TXT,
        _ => return None,
    })
}

/// `ip port` lines, each authorized as a connect target.
fn parse_servers(ctx: &mut dyn CallContext, text: &str) -> Result<Vec<(IpAddr, u16)>, HostError> {
    let mut servers = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let parsed = line
            .split_once(' ')
            .and_then(|(ip, port)| Some((ip.parse::<IpAddr>().ok()?, port.parse::<u16>().ok()?)));
        let (ip, port) = parsed.ok_or_else(|| {
            HostError::TypeError("op_node_dns_query expects `ip port` server lines".into())
        })?;
        authorize(
            ctx,
            "op_node_dns_query",
            Request::NetConnect(&ip.to_string(), port),
        )?;
        servers.push((ip, port));
    }
    Ok(servers)
}

/// Queries to the system resolver reach whoever serves the queried name's zone, so they are a
/// DNS tunnel channel and need `net-connect`. The scope is the queried host on port 53 rather
/// than `*:53`: the system server address is not known to the script, and `--allow-net=host`
/// then lets a script resolve names under hosts the user already trusts, but nothing else.
fn authorize_system_query(ctx: &mut dyn CallContext, name: &str) -> Result<(), HostError> {
    authorize(
        ctx,
        "op_node_dns_query",
        Request::NetConnect(name.trim_end_matches('.'), SYSTEM_DNS_PORT),
    )
}

fn fail(code: &str, detail: impl std::fmt::Display) -> VelyrError {
    VelyrError::new(ErrorCode::Network, format!("{code}|{detail}"))
}

fn dns_error(error: &NetError) -> VelyrError {
    let code = if error.is_nx_domain() {
        "ENOTFOUND"
    } else if error.is_no_records_found() {
        "ENODATA"
    } else if matches!(error, NetError::Timeout) {
        "ETIMEOUT"
    } else {
        "ESERVFAIL"
    };
    fail(code, error)
}

fn build_resolver(servers: &[(IpAddr, u16)]) -> Result<TokioResolver, VelyrError> {
    let builder = if servers.is_empty() {
        TokioResolver::builder_tokio().map_err(|error| dns_error(&error))?
    } else {
        let configs = servers
            .iter()
            .map(|&(ip, port)| {
                let (mut udp, mut tcp) = (ConnectionConfig::udp(), ConnectionConfig::tcp());
                udp.port = port;
                tcp.port = port;
                NameServerConfig::new(ip, true, vec![udp, tcp])
            })
            .collect();
        TokioResolver::builder_with_config(
            ResolverConfig::from_name_servers(configs),
            TokioRuntimeProvider::default(),
        )
    };
    builder.build().map_err(|error| dns_error(&error))
}

fn host(name: &impl std::fmt::Display) -> String {
    name.to_string().trim_end_matches('.').to_string()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The record type name and fields of `data`; `None` for types node:dns has no shape for.
fn fields(data: &RData) -> Option<(&'static str, Json)> {
    Some(match data {
        RData::A(a) => ("A", json!({ "address": a.0.to_string() })),
        RData::AAAA(a) => ("AAAA", json!({ "address": a.0.to_string() })),
        RData::CNAME(n) => ("CNAME", json!({ "value": host(&n.0) })),
        RData::NS(n) => ("NS", json!({ "value": host(&n.0) })),
        RData::PTR(n) => ("PTR", json!({ "value": host(&n.0) })),
        RData::MX(mx) => (
            "MX",
            json!({ "priority": mx.preference, "exchange": host(&mx.exchange) }),
        ),
        RData::TXT(txt) => (
            "TXT",
            json!({ "entries": txt.txt_data.iter().map(|part| text(part)).collect::<Vec<_>>() }),
        ),
        RData::SRV(srv) => (
            "SRV",
            json!({ "priority": srv.priority, "weight": srv.weight, "port": srv.port, "name": host(&srv.target) }),
        ),
        RData::SOA(soa) => (
            "SOA",
            json!({
                "nsname": host(&soa.mname), "hostmaster": host(&soa.rname), "serial": soa.serial,
                "refresh": soa.refresh, "retry": soa.retry, "expire": soa.expire, "minttl": soa.minimum,
            }),
        ),
        RData::NAPTR(n) => (
            "NAPTR",
            json!({
                "order": n.order, "preference": n.preference, "flags": text(&n.flags),
                "service": text(&n.services), "regexp": text(&n.regexp), "replacement": host(&n.replacement),
            }),
        ),
        RData::CAA(caa) => (
            "CAA",
            json!({
                "critical": if caa.issuer_critical { 128 } else { 0 } | u32::from(caa.reserved_flags),
                "tag": caa.tag, "value": text(&caa.value),
            }),
        ),
        _ => return None,
    })
}

async fn query(
    name: String,
    asked: RecordType,
    servers: Vec<(IpAddr, u16)>,
) -> Result<String, VelyrError> {
    let resolver = build_resolver(&servers)?;
    // A trailing dot makes the name fully qualified, so resolv.conf search domains are not applied (like Node).
    let fqdn = if name.ends_with('.') {
        name.clone()
    } else {
        format!("{name}.")
    };
    let lookup = resolver
        .lookup(fqdn.as_str(), asked)
        .await
        .map_err(|error| dns_error(&error))?;
    let mut answers = Vec::new();
    for record in lookup.answers() {
        let Some((kind, Json::Object(mut entry))) = fields(&record.data) else {
            continue;
        };
        if asked != RecordType::ANY && record.data.record_type() != asked {
            continue;
        }
        let mut shaped = Map::new();
        shaped.insert("type".into(), kind.into());
        shaped.insert("ttl".into(), record.ttl.into());
        shaped.append(&mut entry);
        answers.push(Json::Object(shaped));
    }
    if answers.is_empty() {
        return Err(fail("ENODATA", format!("no {asked} records for {name}")));
    }
    Ok(Json::Array(answers).to_string())
}

fn op_dns_query(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let name = string_arg(args, 0)?;
    let rrtype = string_arg(args, 1)?;
    let asked = record_type(&rrtype)
        .ok_or_else(|| HostError::TypeError(format!("unsupported DNS record type {rrtype}")))?;
    let servers = parse_servers(ctx, &string_arg(args, 2)?)?;
    if servers.is_empty() {
        authorize_system_query(ctx, &name)?;
    }
    op_async(ctx, |token| async move {
        tokio::select! {
            () = token.cancelled() => Err(VelyrError::cancelled()),
            answered = query(name, asked, servers) => answered.map(CompletionValue::String),
        }
    })
}

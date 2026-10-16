//! Loopback latency comparison, run on demand:
//! `cargo test -p velyr-io-http --features http3 --release -- --ignored --nocapture h3_vs_tcp`.
//! Loopback has no loss, no RTT and no handshake cost worth speaking of, so this measures stack
//! overhead only; it says nothing about the real-network benefits of QUIC.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::RootCertStore;
use rustls::crypto::aws_lc_rs::default_provider;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use velyr_io_net::Resolver;
use velyr_runtime_permissions::{Capability, CapabilitySet, HostPattern, PortRange};

use super::{H3Connection, client_endpoint, serve_h3, server_endpoint};
use crate::{
    BodyReceiver, ClientOptions, Handler, HttpClient, HttpRequestParts, HttpResponseParts, Limits,
    serve_listener,
};

const REQUESTS: usize = 1000;
const WARMUP: usize = 50;
const PAYLOAD: usize = 1024;

fn handler() -> Handler {
    Arc::new(|_request| {
        Box::pin(async move {
            Ok(HttpResponseParts {
                status: 200,
                headers: vec![],
                body: BodyReceiver::once(vec![b'x'; PAYLOAD]),
                upgrade: None,
            })
        })
    })
}

fn get(url: &str) -> HttpRequestParts {
    HttpRequestParts {
        remote_addr: None,
        method: "GET".into(),
        uri: url.into(),
        version: String::new(),
        headers: vec![],
        body: BodyReceiver::empty(),
    }
}

async fn drain(response: HttpResponseParts) {
    assert_eq!(response.body.collect().await.unwrap().len(), PAYLOAD);
}

fn report(label: &str, mut samples: Vec<Duration>) {
    samples.sort();
    let micros = |d: Duration| d.as_secs_f64() * 1e6;
    let median = micros(samples[samples.len() / 2]);
    let p99 = micros(samples[samples.len() * 99 / 100]);
    println!(
        "{label:<5} n={} median={median:.0}us p99={p99:.0}us",
        samples.len()
    );
}

async fn measure<F, Fut>(label: &str, mut one: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    for _ in 0..WARMUP {
        one().await;
    }
    let mut samples = Vec::with_capacity(REQUESTS);
    for _ in 0..REQUESTS {
        let start = Instant::now();
        one().await;
        samples.push(start.elapsed());
    }
    report(label, samples);
}

fn resolver() -> Resolver {
    Arc::new(|_host, port| {
        Box::pin(async move { Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]) })
    })
}

fn tcp_client(roots: &RootCertStore, alpn: &[u8]) -> HttpClient {
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots.clone())
        .with_no_client_auth();
    tls.alpn_protocols = vec![alpn.to_vec()];
    let permissions = CapabilitySet::builder()
        .allow(Capability::NetConnect(
            HostPattern::new("localhost"),
            PortRange {
                start: 0,
                end: 65535,
            },
        ))
        .build();
    let options = ClientOptions {
        tls: Some(Arc::new(tls)),
        ..ClientOptions::default()
    };
    HttpClient::new(permissions, resolver(), options)
}

#[tokio::test]
#[ignore = "benchmark, run on demand"]
async fn h3_vs_tcp_loopback_latency() {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let mut roots = RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();
    let key = || PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
    let stop = CancellationToken::new();

    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], key())
        .unwrap();
    tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_port = listener.local_addr().unwrap().port();
    tokio::spawn(serve_listener(
        listener,
        Some(Arc::new(tls)),
        Limits::default(),
        handler(),
        stop.clone(),
    ));

    let endpoint = server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        vec![cert.der().clone()],
        key(),
    )
    .unwrap();
    let udp_addr = endpoint.local_addr().unwrap();
    tokio::spawn(serve_h3(
        endpoint,
        Limits::default(),
        handler(),
        stop.clone(),
    ));

    let url = format!("https://localhost:{tcp_port}/");
    for (label, alpn) in [("h1", &b"http/1.1"[..]), ("h2", &b"h2"[..])] {
        let client = tcp_client(&roots, alpn);
        measure(label, || async {
            drain(client.request(get(&url)).await.unwrap()).await;
        })
        .await;
    }

    let h3_endpoint = client_endpoint(roots).unwrap();
    let cold = Instant::now();
    let conn = H3Connection::connect(&h3_endpoint, udp_addr, "localhost")
        .await
        .unwrap();
    drain(conn.request(get("https://localhost/")).await.unwrap()).await;
    println!("h3 cold connect + first request: {:?}", cold.elapsed());
    measure("h3", || async {
        drain(conn.request(get("https://localhost/")).await.unwrap()).await;
    })
    .await;
    stop.cancel();
}

//! Round trips over real loopback UDP sockets.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_util::sync::CancellationToken;

use super::{H3Connection, alt_svc_value, client_endpoint, serve_h3, server_endpoint};
use crate::{BodyReceiver, Handler, HttpRequestParts, HttpResponseParts, Limits};

/// Answers `"{method} {uri} {version} {request body}"` with `x-test: 1`.
fn echo_handler() -> Handler {
    Arc::new(|request: HttpRequestParts| {
        Box::pin(async move {
            let head = format!("{} {} {} ", request.method, request.uri, request.version);
            let body = request.body.collect().await?;
            let mut out = head.into_bytes();
            out.extend_from_slice(&body);
            Ok(HttpResponseParts {
                status: 200,
                headers: vec![("x-test".into(), "1".into())],
                body: BodyReceiver::once(out),
                upgrade: None,
            })
        })
    })
}

struct Fixture {
    addr: SocketAddr,
    roots: RootCertStore,
    stop: CancellationToken,
}

fn start(handler: Handler, limits: Limits) -> Fixture {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
    let chain: Vec<CertificateDer<'static>> = vec![cert.der().clone()];
    let mut roots = RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();

    let endpoint = server_endpoint("127.0.0.1:0".parse().unwrap(), chain, key).unwrap();
    let addr = endpoint.local_addr().unwrap();
    let stop = CancellationToken::new();
    tokio::spawn(serve_h3(endpoint, limits, handler, stop.clone()));
    Fixture { addr, roots, stop }
}

async fn connect(fixture: &Fixture) -> H3Connection {
    let endpoint = client_endpoint(fixture.roots.clone()).unwrap();
    H3Connection::connect(&endpoint, fixture.addr, "localhost")
        .await
        .unwrap()
}

fn request(method: &str, uri: &str, body: &'static str) -> HttpRequestParts {
    HttpRequestParts {
        remote_addr: None,
        method: method.into(),
        uri: uri.into(),
        version: String::new(),
        headers: vec![],
        body: BodyReceiver::once(body),
    }
}

async fn text(response: HttpResponseParts) -> String {
    String::from_utf8(response.body.collect().await.unwrap()).unwrap()
}

#[tokio::test]
async fn get_round_trips_over_h3() {
    let fixture = start(echo_handler(), Limits::default());
    let conn = connect(&fixture).await;

    let response = conn
        .request(request("GET", "https://localhost/hello?a=1", ""))
        .await
        .unwrap();

    assert_eq!(response.status, 200);
    assert!(
        response
            .headers
            .iter()
            .any(|(k, v)| k == "x-test" && v == "1")
    );
    assert_eq!(text(response).await, "GET /hello?a=1 HTTP/3.0 ");
    fixture.stop.cancel();
}

#[tokio::test]
async fn post_body_reaches_the_handler_and_connection_is_reused() {
    let fixture = start(echo_handler(), Limits::default());
    let conn = connect(&fixture).await;

    for i in 0..3 {
        let response = conn
            .request(request("POST", "https://localhost/echo", "ping"))
            .await
            .unwrap();
        assert_eq!(
            text(response).await,
            "POST /echo HTTP/3.0 ping",
            "request {i}"
        );
    }
    fixture.stop.cancel();
}

#[tokio::test]
async fn oversized_request_body_is_rejected_with_413() {
    let limits = Limits {
        max_body_bytes: 3,
        ..Limits::default()
    };
    let fixture = start(echo_handler(), limits);
    let conn = connect(&fixture).await;

    let response = conn
        .request(request("POST", "https://localhost/echo", "too long"))
        .await
        .unwrap();

    assert_eq!(response.status, 413);
    fixture.stop.cancel();
}

#[tokio::test]
async fn client_without_the_server_cert_fails_the_handshake() {
    let fixture = start(echo_handler(), Limits::default());
    let endpoint = client_endpoint(RootCertStore::empty()).unwrap();

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        H3Connection::connect(&endpoint, fixture.addr, "localhost"),
    )
    .await
    .unwrap();

    assert!(result.is_err());
    fixture.stop.cancel();
}

#[test]
fn alt_svc_advertises_h3_on_the_udp_port() {
    assert_eq!(
        alt_svc_value(443, Duration::from_secs(86400)),
        r#"h3=":443"; ma=86400"#
    );
}

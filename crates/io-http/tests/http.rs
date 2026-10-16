//! Server and client over real loopback sockets: h1, h2 (prior knowledge and TLS ALPN), permission
//! denial before any socket, body limits, header timeout and pooling.

use std::net::{SocketAddr, TcpListener as StdListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rustls::RootCertStore;
use rustls::crypto::aws_lc_rs::default_provider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use velyr_io_http::{
    BodyReceiver, ClientOptions, Handler, HttpClient, HttpRequestParts, HttpResponseParts, Limits,
    body_channel, serve_listener,
};
use velyr_io_net::Resolver;
use velyr_runtime_errors::ErrorCode;
use velyr_runtime_permissions::{Capability, CapabilitySet, HostPattern, PortRange};
use velyr_runtime_resources::{QuotaConfig, QuotaContext};

const ALL_PORTS: PortRange = PortRange {
    start: 0,
    end: 65535,
};

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

/// Responds with `size` bytes in chunks of 10.
fn blob_handler(size: usize) -> Handler {
    Arc::new(move |_request| {
        Box::pin(async move {
            let (tx, rx) = body_channel();
            tokio::spawn(async move {
                let mut left = size;
                while left > 0 {
                    let n = left.min(10);
                    if !tx.send(Ok(vec![b'x'; n].into())).await {
                        return;
                    }
                    left -= n;
                }
            });
            Ok(HttpResponseParts {
                status: 200,
                headers: vec![],
                body: rx,
                upgrade: None,
            })
        })
    })
}

struct Cert {
    cert_der: CertificateDer<'static>,
    key_der: Vec<u8>,
}

fn self_signed() -> Cert {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    Cert {
        cert_der: cert.der().clone(),
        key_der: signing_key.serialize_der(),
    }
}

fn server_tls(cert: &Cert) -> Arc<rustls::ServerConfig> {
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.key_der.clone()));
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert_der.clone()], key)
        .unwrap();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Arc::new(config)
}

fn client_tls(cert: &Cert, alpn: &[&[u8]]) -> Arc<rustls::ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.add(cert.cert_der.clone()).unwrap();
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Arc::new(config)
}

async fn start_server(
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: Limits,
    handler: Handler,
) -> (u16, CancellationToken) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shutdown = CancellationToken::new();
    tokio::spawn(serve_listener(
        listener,
        tls,
        limits,
        handler,
        shutdown.clone(),
    ));
    (port, shutdown)
}

fn permissions(hosts: &[&str]) -> Arc<CapabilitySet> {
    let mut builder = CapabilitySet::builder();
    for host in hosts {
        builder = builder.allow(Capability::NetConnect(HostPattern::new(*host), ALL_PORTS));
    }
    builder.build()
}

fn localhost_resolver() -> Resolver {
    Arc::new(|_host, port| {
        Box::pin(async move { Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]) })
    })
}

fn client(hosts: &[&str], options: ClientOptions) -> HttpClient {
    HttpClient::new(permissions(hosts), localhost_resolver(), options)
}

fn get(url: &str) -> HttpRequestParts {
    request("GET", url, BodyReceiver::empty())
}

fn request(method: &str, url: &str, body: BodyReceiver) -> HttpRequestParts {
    HttpRequestParts {
        remote_addr: None,
        method: method.into(),
        uri: url.into(),
        version: String::new(),
        headers: vec![],
        body,
    }
}

async fn text(response: HttpResponseParts) -> String {
    String::from_utf8(response.body.collect().await.unwrap()).unwrap()
}

#[tokio::test]
async fn h1_request_gets_a_response() {
    let (port, _stop) = start_server(None, Limits::default(), echo_handler()).await;
    let client = client(&["127.0.0.1"], ClientOptions::default());
    let response = client
        .request(request(
            "POST",
            &format!("http://127.0.0.1:{port}/echo?a=1"),
            BodyReceiver::once("ping"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert!(
        response
            .headers
            .iter()
            .any(|(k, v)| k == "x-test" && v == "1")
    );
    assert_eq!(text(response).await, "POST /echo?a=1 HTTP/1.1 ping");
}

#[tokio::test]
async fn h2_prior_knowledge_request_gets_a_response() {
    let (port, _stop) = start_server(None, Limits::default(), echo_handler()).await;
    let client = client(
        &["127.0.0.1"],
        ClientOptions {
            http2_prior_knowledge: true,
            ..ClientOptions::default()
        },
    );
    let response = client
        .request(get(&format!("http://127.0.0.1:{port}/h2")))
        .await
        .unwrap();
    assert_eq!(text(response).await, "GET /h2 HTTP/2.0 ");
}

#[tokio::test]
async fn tls_alpn_selects_h2_or_h1() {
    let cert = self_signed();
    let (port, _stop) =
        start_server(Some(server_tls(&cert)), Limits::default(), echo_handler()).await;
    let url = format!("https://localhost:{port}/tls");
    let cases: [(&[&[u8]], &str); 2] = [
        (&[b"h2", b"http/1.1"], "HTTP/2.0"),
        (&[b"http/1.1"], "HTTP/1.1"),
    ];
    for (alpn, expected) in cases {
        let client = client(
            &["localhost"],
            ClientOptions {
                tls: Some(client_tls(&cert, alpn)),
                ..ClientOptions::default()
            },
        );
        let response = client.request(get(&url)).await.unwrap();
        assert_eq!(text(response).await, format!("GET /tls {expected} "));
    }
}

#[tokio::test]
async fn connect_without_grant_is_denied_before_any_socket() {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepts = Arc::new(AtomicUsize::new(0));
    let counter = accepts.clone();
    std::thread::spawn(move || {
        for _ in listener.incoming() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    let client = client(&[], ClientOptions::default());
    let err = client
        .request(get(&format!("http://127.0.0.1:{port}/")))
        .await
        .err()
        .expect("denied");
    assert_eq!(err.code(), ErrorCode::PermissionDenied);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(accepts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn request_body_over_the_limit_is_413() {
    let limits = Limits {
        max_body_bytes: 16,
        ..Limits::default()
    };
    let (port, _stop) = start_server(None, limits, echo_handler()).await;
    let client = client(&["127.0.0.1"], ClientOptions::default());
    let url = format!("http://127.0.0.1:{port}/");
    let big = client
        .request(request("POST", &url, BodyReceiver::once(vec![b'a'; 100])))
        .await
        .unwrap();
    assert_eq!(big.status, 413);
    let small = client
        .request(request("POST", &url, BodyReceiver::once("tiny")))
        .await
        .unwrap();
    assert_eq!(small.status, 200);
}

#[tokio::test]
async fn streamed_request_body_over_the_limit_errors_in_the_handler() {
    let limits = Limits {
        max_body_bytes: 16,
        ..Limits::default()
    };
    let (port, _stop) = start_server(None, limits, echo_handler()).await;
    let client = client(&["127.0.0.1"], ClientOptions::default());
    let (tx, rx) = body_channel();
    tokio::spawn(async move {
        for _ in 0..10 {
            if !tx.send(Ok(vec![b'a'; 10].into())).await {
                return;
            }
        }
    });
    let response = client
        .request(request("POST", &format!("http://127.0.0.1:{port}/"), rx))
        .await
        .unwrap();
    assert_eq!(response.status, 500);
}

#[tokio::test]
async fn response_body_over_the_limit_is_quota_exceeded() {
    let (port, _stop) = start_server(None, Limits::default(), blob_handler(100)).await;
    let client = client(
        &["127.0.0.1"],
        ClientOptions {
            limits: Limits {
                max_body_bytes: 32,
                ..Limits::default()
            },
            ..ClientOptions::default()
        },
    );
    let response = client
        .request(get(&format!("http://127.0.0.1:{port}/")))
        .await
        .unwrap();
    let err = response.body.collect().await.expect_err("limit hit");
    assert_eq!(err.code(), ErrorCode::QuotaExceeded);
}

#[tokio::test]
async fn body_bytes_quota_limits_in_flight_bytes() {
    let (port, _stop) = start_server(None, Limits::default(), blob_handler(100)).await;
    let quotas = Arc::new(QuotaContext::new(QuotaConfig {
        body_bytes: Some(40),
        ..QuotaConfig::default()
    }));
    let client = client(
        &["127.0.0.1"],
        ClientOptions {
            limits: Limits {
                quotas: Some(quotas.clone()),
                ..Limits::default()
            },
            ..ClientOptions::default()
        },
    );
    let response = client
        .request(get(&format!("http://127.0.0.1:{port}/")))
        .await
        .unwrap();
    let err = response.body.collect().await.expect_err("quota hit");
    assert_eq!(err.code(), ErrorCode::QuotaExceeded);
}

#[tokio::test]
async fn silent_connection_is_closed_after_the_header_timeout() {
    let limits = Limits {
        header_timeout: Duration::from_millis(100),
        ..Limits::default()
    };
    let (port, _stop) = start_server(None, limits, echo_handler()).await;
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let mut buf = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buf))
        .await
        .expect("server closed the idle connection");
    assert!(matches!(read, Ok(0) | Err(_)));
    let _ = socket.write_all(b"").await;
}

#[tokio::test]
async fn sequential_requests_reuse_one_pooled_connection() {
    let (port, _stop) = start_server(
        None,
        Limits::default(),
        Arc::new(|request: HttpRequestParts| {
            Box::pin(async move {
                Ok(HttpResponseParts {
                    status: 200,
                    headers: vec![],
                    body: BodyReceiver::once(request.remote_addr.unwrap().port().to_string()),
                    upgrade: None,
                })
            })
        }),
    )
    .await;
    let client = client(&["127.0.0.1"], ClientOptions::default());
    let url = format!("http://127.0.0.1:{port}/");
    let first = text(client.request(get(&url)).await.unwrap()).await;
    let second = text(client.request(get(&url)).await.unwrap()).await;
    assert_eq!(first, second);
}

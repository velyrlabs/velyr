//! Header limits (431), idle and handshake timeouts, and graceful shutdown, over loopback sockets.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use velyr_io_http::{
    BodyReceiver, ClientOptions, Handler, HttpClient, HttpRequestParts, HttpResponseParts, Limits,
    serve_listener, serve_listener_graceful,
};
use velyr_io_net::Resolver;
use velyr_runtime_permissions::{Capability, CapabilitySet, HostPattern, PortRange};

fn handler(delay: Duration) -> Handler {
    Arc::new(move |_request: HttpRequestParts| {
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Ok(HttpResponseParts {
                status: 200,
                headers: vec![],
                body: BodyReceiver::once("ok"),
                upgrade: None,
            })
        })
    })
}

async fn bind() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

async fn start(limits: Limits) -> (u16, CancellationToken) {
    let (listener, port) = bind().await;
    let stop = CancellationToken::new();
    tokio::spawn(serve_listener(
        listener,
        None,
        limits,
        handler(Duration::ZERO),
        stop.clone(),
    ));
    (port, stop)
}

fn client(limits: Limits) -> HttpClient {
    let permissions = CapabilitySet::builder()
        .allow(Capability::NetConnect(
            HostPattern::new("127.0.0.1"),
            PortRange {
                start: 0,
                end: 65535,
            },
        ))
        .build();
    let resolver: Resolver = Arc::new(|_host, port| {
        Box::pin(async move { Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]) })
    });
    HttpClient::new(
        permissions,
        resolver,
        ClientOptions {
            limits,
            ..ClientOptions::default()
        },
    )
}

fn get(url: &str, headers: Vec<(String, String)>) -> HttpRequestParts {
    HttpRequestParts {
        remote_addr: None,
        method: "GET".into(),
        uri: url.into(),
        version: String::new(),
        headers,
        body: BodyReceiver::empty(),
    }
}

/// Reads until EOF or error, bounded; returns what arrived and whether the peer closed.
async fn drain(socket: &mut TcpStream, within: Duration) -> (Vec<u8>, bool) {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, socket.read(&mut buf)).await {
            Err(_) => return (data, false),
            Ok(Ok(0) | Err(_)) => return (data, true),
            Ok(Ok(n)) => data.extend_from_slice(&buf[..n]),
        }
    }
}

const BIG_HEADER_LIMITS: Limits = Limits {
    max_header_bytes: 16 * 1024,
    max_body_bytes: 16 * 1024 * 1024,
    header_timeout: Duration::from_secs(30),
    idle_timeout: Duration::from_secs(60),
    request_timeout: Duration::from_secs(300),
    body_idle_timeout: Duration::from_secs(30),
    quotas: None,
};

#[tokio::test]
async fn request_header_above_the_limit_is_431() {
    let (port, _stop) = start(BIG_HEADER_LIMITS).await;
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!(
        "GET / HTTP/1.1\r\nhost: x\r\nx-big: {}\r\n\r\n",
        "a".repeat(40_000)
    );
    let _ = socket.write_all(request.as_bytes()).await;
    let (data, _) = drain(&mut socket, Duration::from_secs(3)).await;
    assert!(
        String::from_utf8_lossy(&data).starts_with("HTTP/1.1 431"),
        "{data:?}"
    );
}

#[tokio::test]
async fn client_sees_431_for_an_oversized_request_header() {
    let (port, _stop) = start(BIG_HEADER_LIMITS).await;
    let big = vec![("x-big".to_owned(), "a".repeat(40_000))];
    let result = client(Limits::default())
        .request(get(&format!("http://127.0.0.1:{port}/"), big))
        .await;
    // Either the 431 arrived, or the server closed while the client was still writing.
    assert!(result.map_or(true, |response| response.status == 431));
}

#[tokio::test]
async fn client_errors_on_a_response_head_above_the_limit() {
    let (listener, port) = bind().await;
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = socket.read(&mut buf).await;
        let head = format!(
            "HTTP/1.1 200 OK\r\nx-big: {}\r\ncontent-length: 0\r\n\r\n",
            "a".repeat(40_000)
        );
        let _ = socket.write_all(head.as_bytes()).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let limits = Limits {
        max_header_bytes: 8192,
        ..Limits::default()
    };
    let result = client(limits)
        .request(get(&format!("http://127.0.0.1:{port}/"), vec![]))
        .await;
    assert!(result.is_err());
}

const GET: &[u8] = b"GET / HTTP/1.1\r\nhost: x\r\n\r\n";

#[tokio::test]
async fn idle_h1_connection_is_closed_after_idle_timeout() {
    let limits = Limits {
        header_timeout: Duration::from_secs(30),
        idle_timeout: Duration::from_millis(300),
        ..Limits::default()
    };
    let (port, _stop) = start(limits).await;
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(GET).await.unwrap();
    let started = Instant::now();
    let (data, closed) = drain(&mut socket, Duration::from_secs(5)).await;
    assert!(String::from_utf8_lossy(&data).starts_with("HTTP/1.1 200"));
    assert!(closed, "idle keep-alive connection stayed open");
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn idle_timeout_is_independent_of_the_header_timeout() {
    let limits = Limits {
        header_timeout: Duration::from_millis(200),
        idle_timeout: Duration::from_secs(3),
        ..Limits::default()
    };
    let (port, _stop) = start(limits).await;
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(GET).await.unwrap();
    let (_, closed) = drain(&mut socket, Duration::from_millis(1000)).await;
    assert!(
        !closed,
        "keep-alive connection closed by the header timeout"
    );
}

#[tokio::test]
async fn slow_head_is_still_cut_by_the_header_timeout() {
    let limits = Limits {
        header_timeout: Duration::from_millis(300),
        idle_timeout: Duration::from_secs(30),
        ..Limits::default()
    };
    let (port, _stop) = start(limits).await;
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket
        .write_all(b"GET / HTTP/1.1\r\nhost: x\r\n")
        .await
        .unwrap();
    let (_, closed) = drain(&mut socket, Duration::from_secs(5)).await;
    assert!(closed, "half-sent head kept the connection open");
}

#[tokio::test]
async fn h2_handshake_that_never_finishes_is_closed() {
    let limits = Limits {
        header_timeout: Duration::from_millis(300),
        idle_timeout: Duration::from_secs(30),
        ..Limits::default()
    };
    let (port, _stop) = start(limits).await;
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket
        .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .unwrap();
    let (_, closed) = drain(&mut socket, Duration::from_secs(5)).await;
    assert!(
        closed,
        "h2 preface without SETTINGS kept the connection open"
    );
}

#[tokio::test]
async fn shutdown_lets_an_in_flight_request_finish() {
    let (listener, port) = bind().await;
    let stop = CancellationToken::new();
    let server = tokio::spawn(serve_listener_graceful(
        listener,
        None,
        Limits::default(),
        handler(Duration::from_millis(500)),
        stop.clone(),
        Duration::from_secs(5),
    ));
    let url = format!("http://127.0.0.1:{port}/");
    let pending =
        tokio::spawn(async move { client(Limits::default()).request(get(&url, vec![])).await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    stop.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !server.is_finished(),
        "server stopped before draining the request"
    );
    let response = pending.await.unwrap().expect("in-flight request completes");
    assert_eq!(response.status, 200);
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("server stopped within the grace period")
        .unwrap();
}

#[tokio::test]
async fn shutdown_gives_up_on_a_stuck_request_after_the_grace_period() {
    let (listener, port) = bind().await;
    let stop = CancellationToken::new();
    let server = tokio::spawn(serve_listener_graceful(
        listener,
        None,
        Limits::default(),
        handler(Duration::from_secs(3600)),
        stop.clone(),
        Duration::from_millis(300),
    ));
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(GET).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let started = Instant::now();
    stop.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !server.is_finished(),
        "server stopped without waiting for the grace period"
    );
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("server stopped despite the stuck request")
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
}

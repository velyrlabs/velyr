//! HTTP/2 rapid-reset bound (pinned) and the slow-body gap in `Activity::verdict` (RT-06 closes it).

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use velyr_io_http::{
    BodyReceiver, Handler, HttpRequestParts, HttpResponseParts, Limits, serve_listener,
};

const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const FRAME_HEADERS: u8 = 1;
const FRAME_RST_STREAM: u8 = 3;
const FRAME_GOAWAY: u8 = 7;
const END_STREAM_AND_HEADERS: u8 = 0x5;
const CANCEL: u32 = 8;
const RESET_PAIRS: u32 = 2000;

fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).unwrap().to_be_bytes();
    let mut out = vec![len[1], len[2], len[3], kind, flags];
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// HPACK: `:method GET`, `:scheme http`, `:path /`, `:authority x`.
fn get_headers_block() -> Vec<u8> {
    vec![0x82, 0x86, 0x84, 0x01, 0x01, b'x']
}

fn ok_handler() -> Handler {
    Arc::new(|_request: HttpRequestParts| {
        Box::pin(async {
            Ok(HttpResponseParts {
                status: 200,
                headers: vec![],
                body: BodyReceiver::once("ok"),
                upgrade: None,
            })
        })
    })
}

/// Reads the whole request body, then answers 200.
fn body_reading_handler() -> Handler {
    Arc::new(|request: HttpRequestParts| {
        Box::pin(async move {
            let _ = request.body.collect().await;
            Ok(HttpResponseParts {
                status: 200,
                headers: vec![],
                body: BodyReceiver::once("ok"),
                upgrade: None,
            })
        })
    })
}

/// Never produces a response.
fn stalled_handler() -> Handler {
    Arc::new(|_request: HttpRequestParts| Box::pin(std::future::pending()))
}

async fn start(limits: Limits, handler: Handler) -> (u16, CancellationToken) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    tokio::spawn(serve_listener(
        listener,
        None,
        limits,
        handler,
        stop.clone(),
    ));
    (port, stop)
}

fn short_idle_limits() -> Limits {
    Limits {
        idle_timeout: Duration::from_secs(1),
        ..Limits::default()
    }
}

/// Returns true once the server sent GOAWAY or closed the socket.
async fn server_cut_off(mut read: tokio::net::tcp::OwnedReadHalf) {
    let mut buf = [0u8; 16 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        match read.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => pending.extend_from_slice(&buf[..n]),
        }
        while pending.len() >= 9 {
            let len = usize::from(pending[0]) << 16
                | usize::from(pending[1]) << 8
                | usize::from(pending[2]);
            if pending.len() < 9 + len {
                break;
            }
            if pending[3] == FRAME_GOAWAY {
                return;
            }
            pending.drain(..9 + len);
        }
    }
}

#[tokio::test]
async fn h2_rapid_reset_is_bounded() {
    let (port, _stop) = start(Limits::default(), ok_handler()).await;
    let socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (read, mut write) = socket.into_split();
    let cut = tokio::spawn(server_cut_off(read));
    let block = get_headers_block();
    let mut burst = H2_PREFACE.to_vec();
    burst.extend(frame(4, 0, 0, &[]));
    for pair in 0..RESET_PAIRS {
        let stream = 2 * pair + 1;
        burst.extend(frame(FRAME_HEADERS, END_STREAM_AND_HEADERS, stream, &block));
        burst.extend(frame(FRAME_RST_STREAM, 0, stream, &CANCEL.to_be_bytes()));
    }
    // The server may close before reading everything; only its decision is asserted.
    let _ = write.write_all(&burst).await;

    let outcome = tokio::time::timeout(Duration::from_secs(5), cut).await;

    assert!(
        outcome.is_ok(),
        "server tolerated {RESET_PAIRS} reset pairs without GOAWAY or close"
    );
}

/// Returns how long the server took to cut the connection, or `None` if it never did within `within`.
async fn time_to_cut(mut socket: TcpStream, trickle: bool, within: Duration) -> Option<Duration> {
    let started = Instant::now();
    let mut buf = [0u8; 1024];
    let mut ticker = tokio::time::interval(Duration::from_millis(200));
    loop {
        tokio::select! {
            read = socket.read(&mut buf) => match read {
                Ok(0) | Err(_) => return Some(started.elapsed()),
                Ok(_) => {}
            },
            _ = ticker.tick() => {
                if started.elapsed() >= within {
                    return None;
                }
                if trickle && socket.write_all(b"x").await.is_err() {
                    return Some(started.elapsed());
                }
            }
        }
    }
}

#[tokio::test]
#[ignore = "RT-06: Activity::verdict returns Keep while a request is in flight"]
async fn h1_slow_body_is_cut() {
    let (port, _stop) = start(short_idle_limits(), body_reading_handler()).await;
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket
        .write_all(b"POST / HTTP/1.1\r\nhost: x\r\ncontent-length: 1000000\r\n\r\n")
        .await
        .unwrap();

    let cut = time_to_cut(socket, true, Duration::from_secs(5)).await;

    assert!(
        cut.is_some(),
        "trickled body kept the connection open past 5 s"
    );
}

#[tokio::test]
#[ignore = "RT-06: Activity::verdict returns Keep while a request is in flight"]
async fn stalled_handler_is_cut() {
    let limits = short_idle_limits();
    let budget = limits.idle_timeout + Duration::from_secs(2);
    let (port, _stop) = start(limits, stalled_handler()).await;
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket
        .write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n")
        .await
        .unwrap();

    let cut = time_to_cut(socket, false, budget).await;

    assert!(
        cut.is_some(),
        "stalled handler kept the connection open past {budget:?}"
    );
}

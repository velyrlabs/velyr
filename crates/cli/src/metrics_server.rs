//! The `--metrics-addr` Prometheus exporter. It runs on its own thread with its own Tokio
//! runtime and only reads the atomics in `velyr_runtime_core::metrics`, so it answers even while
//! the isolate thread is stuck in JS. It is an operator facility, not application code: it is
//! not gated by `net-listen`.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use velyr_runtime_core::metrics;

const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
/// A scraper that does not finish its request head in this time is dropped.
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);

/// Binds `addr` now (so a bad address fails the command) and serves `GET /metrics` from a
/// background thread for the rest of the process. Returns the bound address.
pub fn start(addr: SocketAddr) -> std::io::Result<SocketAddr> {
    let listener = std::net::TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    let bound = listener.local_addr()?;
    std::thread::Builder::new()
        .name("velyr-metrics".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .expect("metrics runtime");
            runtime.block_on(serve(listener));
        })?;
    Ok(bound)
}

async fn serve(listener: std::net::TcpListener) {
    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
        return;
    };
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(async move {
            let _ = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEAD_TIMEOUT)
                .serve_connection(TokioIo::new(stream), service_fn(respond))
                .await;
        });
    }
}

async fn respond(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let mut response = Response::new(Full::new(Bytes::new()));
    let method = request.method();
    if request.uri().path() != "/metrics" {
        *response.status_mut() = StatusCode::NOT_FOUND;
    } else if method != Method::GET && method != Method::HEAD {
        *response.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
    } else {
        response
            .headers_mut()
            .insert("content-type", CONTENT_TYPE.parse().expect("static header"));
        if method == Method::GET {
            *response.body_mut() =
                Full::new(Bytes::from(metrics::global().snapshot().to_prometheus()));
        }
    }
    Ok(response)
}

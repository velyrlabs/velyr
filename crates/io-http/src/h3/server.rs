//! h3 accept loop. Each connection and each request stream runs in its own task; the handler is the
//! same [`Handler`] the TCP server uses, so request/response semantics are shared.

use std::net::SocketAddr;

use bytes::{Buf, Bytes};
use h3::error::Code;
use http::{HeaderName, HeaderValue, Response, StatusCode};
use tokio_util::sync::CancellationToken;
use velyr_runtime_errors::VelyrError;

use super::VERSION;
use crate::{Handler, HttpRequestParts, HttpResponseParts, Limits, body_channel};

type Stream = h3::server::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>;
type RecvStream = h3::server::RequestStream<h3_quinn::RecvStream, Bytes>;

/// Accepts QUIC connections on `endpoint` until `shutdown` is cancelled, then closes the endpoint.
pub async fn serve_h3(
    endpoint: quinn::Endpoint,
    limits: Limits,
    handler: Handler,
    shutdown: CancellationToken,
) {
    loop {
        let incoming = tokio::select! {
            _ = shutdown.cancelled() => break,
            incoming = endpoint.accept() => match incoming {
                Some(incoming) => incoming,
                None => break,
            },
        };
        tokio::spawn(serve_connection(incoming, limits.clone(), handler.clone()));
    }
    endpoint.close(0u32.into(), b"shutdown");
}

async fn serve_connection(incoming: quinn::Incoming, limits: Limits, handler: Handler) {
    let Ok(connection) = incoming.await else {
        return;
    };
    let remote = connection.remote_address();
    let built = h3::server::builder()
        .max_field_section_size(limits.max_header_bytes as u64)
        .build(h3_quinn::Connection::new(connection))
        .await;
    let Ok(mut h3) = built else {
        return;
    };
    while let Ok(Some(resolver)) = h3.accept().await {
        let (limits, handler) = (limits.clone(), handler.clone());
        tokio::spawn(async move {
            if let Ok((request, stream)) = resolver.resolve_request().await {
                serve_request(request, stream, remote, limits, handler).await;
            }
        });
    }
}

async fn serve_request(
    request: http::Request<()>,
    stream: Stream,
    remote: SocketAddr,
    limits: Limits,
    handler: Handler,
) {
    let (mut send, recv) = stream.split();
    let declared = request
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    if declared.is_some_and(|n| n > limits.max_body_bytes) {
        reply_status(&mut send, StatusCode::PAYLOAD_TOO_LARGE).await;
        return;
    }

    let (tx, body) = body_channel();
    tokio::spawn(pump_request_body(recv, tx, limits.max_body_bytes));
    let parts = HttpRequestParts {
        remote_addr: Some(remote),
        method: request.method().to_string(),
        uri: request
            .uri()
            .path_and_query()
            .map_or("/", |pq| pq.as_str())
            .to_string(),
        version: VERSION.to_string(),
        headers: request_headers(&request),
        body,
    };
    match handler(parts).await {
        Ok(response) => send_response(&mut send, response).await,
        Err(err) => {
            tracing::warn!(code = ?err.code(), "h3 handler failed");
            reply_status(&mut send, StatusCode::INTERNAL_SERVER_ERROR).await;
        }
    }
}

/// h3 carries the host in `:authority`; handlers expect a `host` header like on h1/h2.
fn request_headers(request: &http::Request<()>) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
        .collect();
    let has_host = headers.iter().any(|(k, _)| k == "host");
    if let (false, Some(authority)) = (has_host, request.uri().authority()) {
        headers.push(("host".into(), authority.to_string()));
    }
    headers
}

async fn pump_request_body(mut recv: RecvStream, tx: crate::BodySender, max_body_bytes: u64) {
    let mut total = 0u64;
    loop {
        let chunk = match recv.recv_data().await {
            Ok(Some(mut buf)) => buf.copy_to_bytes(buf.remaining()),
            Ok(None) => return,
            Err(_) => {
                tx.send(Err(VelyrError::cancelled())).await;
                return;
            }
        };
        total += chunk.len() as u64;
        if total > max_body_bytes {
            tx.send(Err(VelyrError::quota_exceeded("body_bytes"))).await;
            recv.stop_sending(Code::H3_REQUEST_REJECTED);
            return;
        }
        if !tx.send(Ok(chunk)).await {
            recv.stop_sending(Code::H3_REQUEST_CANCELLED);
            return;
        }
    }
}

async fn reply_status(
    send: &mut h3::server::RequestStream<h3_quinn::SendStream<Bytes>, Bytes>,
    status: StatusCode,
) {
    let response = Response::builder().status(status).body(()).ok();
    if let Some(response) = response
        && send.send_response(response).await.is_ok()
    {
        let _ = send.finish().await;
    }
}

async fn send_response(
    send: &mut h3::server::RequestStream<h3_quinn::SendStream<Bytes>, Bytes>,
    parts: HttpResponseParts,
) {
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(parts.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    for (name, value) in &parts.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            builder = builder.header(name, value);
        }
    }
    let Ok(head) = builder.body(()) else {
        return;
    };
    if send.send_response(head).await.is_err() {
        return;
    }
    let mut body = parts.body;
    while let Some(chunk) = body.recv().await {
        let sent = match chunk {
            Ok(bytes) => send.send_data(bytes).await.is_ok(),
            Err(_) => false,
        };
        if !sent {
            send.stop_stream(Code::H3_INTERNAL_ERROR);
            return;
        }
    }
    let _ = send.finish().await;
}

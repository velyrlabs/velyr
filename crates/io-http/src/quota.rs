//! Connection-level `sockets` quota: one unit per accepted connection, held until it closes.

use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use velyr_runtime_resources::{QuotaClass, QuotaContext, QuotaGuard};

use crate::Limits;

const OVER_QUOTA: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\nretry-after: 1\r\nconnection: close\r\ncontent-length: 0\r\n\r\n";

/// A unit for a new connection, or `None` when there is no quota to enforce. `Err` means the
/// connection must be turned away with [`reject`].
pub(crate) fn reserve(limits: &Limits) -> Result<Option<QuotaGuard>, ()> {
    let quotas: &Arc<QuotaContext> = match &limits.quotas {
        Some(quotas) => quotas,
        None => return Ok(None),
    };
    quotas
        .reserve(QuotaClass::Sockets, 1)
        .map(Some)
        .map_err(|_| ())
}

/// Answers a connection over quota with a 503 and closes it, without reading anything.
pub(crate) fn reject(mut tcp: TcpStream) {
    tokio::spawn(async move {
        let _ = tcp.write_all(OVER_QUOTA).await;
        let _ = tcp.shutdown().await;
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;
    use tokio_util::sync::CancellationToken;
    use velyr_runtime_resources::QuotaConfig;

    use super::*;
    use crate::{BodyReceiver, HttpResponseParts, serve_listener_graceful};

    async fn exchange(port: u16, connection: &str) -> (TcpStream, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let request = format!("GET / HTTP/1.1\r\nHost: t\r\nConnection: {connection}\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut text = Vec::new();
        let mut buf = [0u8; 512];
        // `keep-alive` replies end with the body; `close` ones end with EOF.
        while !String::from_utf8_lossy(&text).contains("ok") && !text.ends_with(b"\r\n\r\n") {
            match stream.read(&mut buf).await.unwrap() {
                0 => break,
                n => text.extend_from_slice(&buf[..n]),
            }
        }
        (stream, String::from_utf8_lossy(&text).into_owned())
    }

    #[tokio::test]
    async fn connections_over_the_quota_get_a_503_and_free_units_come_back() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let quotas = Arc::new(QuotaContext::new(QuotaConfig {
            sockets: Some(1),
            ..QuotaConfig::default()
        }));
        let limits = Limits {
            quotas: Some(quotas.clone()),
            ..Limits::default()
        };
        let handler: crate::Handler = Arc::new(|_| {
            Box::pin(async {
                Ok(HttpResponseParts {
                    status: 200,
                    headers: vec![],
                    body: BodyReceiver::once("ok"),
                    upgrade: None,
                })
            })
        });
        let stop = CancellationToken::new();
        tokio::spawn(serve_listener_graceful(
            listener,
            None,
            limits,
            handler,
            stop.clone(),
            Duration::from_secs(1),
        ));
        let (first, text) = exchange(port, "keep-alive").await;
        assert!(text.starts_with("HTTP/1.1 200"), "{text}");
        let (_, rejected) = exchange(port, "close").await;
        assert!(rejected.starts_with("HTTP/1.1 503"), "{rejected}");
        drop(first);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let (_, text) = exchange(port, "close").await;
            if text.starts_with("HTTP/1.1 200") {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "unit never released: {text}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        stop.cancel();
    }
}

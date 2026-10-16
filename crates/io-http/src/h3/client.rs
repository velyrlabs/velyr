//! One h3 connection to one server; requests multiplex over it as separate QUIC streams.

use std::net::SocketAddr;

use bytes::{Buf, Bytes};
use http::{HeaderName, HeaderValue, Request, Uri};
use tokio::task::JoinHandle;
use velyr_runtime_errors::{ErrorCode, VelyrError};

use crate::{HttpRequestParts, HttpResponseParts, Limits, body_channel};

fn network(err: impl std::fmt::Display) -> VelyrError {
    VelyrError::new(ErrorCode::Network, err.to_string())
}

pub struct H3Connection {
    send_request: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
    limits: Limits,
    driver: JoinHandle<()>,
}

impl Drop for H3Connection {
    fn drop(&mut self) {
        // The driver owns the connection; ending it closes the QUIC connection.
        self.driver.abort();
    }
}

impl H3Connection {
    /// Handshakes with `addr`, verifying the certificate against `server_name`.
    pub async fn connect(
        endpoint: &quinn::Endpoint,
        addr: SocketAddr,
        server_name: &str,
    ) -> Result<Self, VelyrError> {
        let connection = endpoint
            .connect(addr, server_name)
            .map_err(network)?
            .await
            .map_err(network)?;
        let (mut driver, send_request) = h3::client::new(h3_quinn::Connection::new(connection))
            .await
            .map_err(network)?;
        let driver = tokio::spawn(async move {
            let _ = driver.wait_idle().await;
        });
        Ok(Self {
            send_request,
            limits: Limits::default(),
            driver,
        })
    }

    /// Sends `request` (`uri` is an absolute https URL) and returns once the response head arrived;
    /// the body keeps streaming through the returned channel. The request body is buffered first
    /// (prototype: no streaming uploads).
    pub async fn request(
        &self,
        request: HttpRequestParts,
    ) -> Result<HttpResponseParts, VelyrError> {
        let uri: Uri = request
            .uri
            .parse()
            .map_err(|_| VelyrError::invalid_data("invalid request url"))?;
        let body = request.body.collect().await?;
        let mut builder = Request::builder().method(request.method.as_str()).uri(uri);
        for (name, value) in &request.headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                builder = builder.header(name, value);
            }
        }
        if !body.is_empty() {
            builder = builder.header(http::header::CONTENT_LENGTH, body.len());
        }
        let head = builder
            .body(())
            .map_err(|_| VelyrError::invalid_data("invalid request"))?;

        let mut stream = self
            .send_request
            .clone()
            .send_request(head)
            .await
            .map_err(network)?;
        if !body.is_empty() {
            stream.send_data(Bytes::from(body)).await.map_err(network)?;
        }
        stream.finish().await.map_err(network)?;

        let response = tokio::time::timeout(self.limits.header_timeout, stream.recv_response())
            .await
            .map_err(|_| VelyrError::timeout("no response head before the header timeout"))?
            .map_err(network)?;

        let (tx, rx) = body_channel();
        let max = self.limits.max_body_bytes;
        tokio::spawn(async move {
            let mut total = 0u64;
            while let Ok(Some(mut buf)) = stream.recv_data().await {
                let chunk = buf.copy_to_bytes(buf.remaining());
                total += chunk.len() as u64;
                if total > max {
                    tx.send(Err(VelyrError::quota_exceeded("body_bytes"))).await;
                    return;
                }
                if !tx.send(Ok(chunk)).await {
                    return;
                }
            }
        });
        Ok(HttpResponseParts {
            status: response.status().as_u16(),
            headers: response
                .headers()
                .iter()
                .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
                .collect(),
            body: rx,
            upgrade: None,
        })
    }
}

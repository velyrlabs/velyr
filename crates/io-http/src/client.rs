use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use http::{HeaderName, HeaderValue, Method, Request, Uri};
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;
use tower_service::Service;
use velyr_io_net::{Duplex, Resolver, dial};
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_permissions::CapabilitySet;

use crate::body::pump;
use crate::{BodyReceiver, HttpRequestParts, HttpResponseParts, Limits, body_channel};

const DEFAULT_POOL_IDLE: Duration = Duration::from_secs(90);

#[derive(Clone, Default)]
pub struct ClientOptions {
    pub limits: Limits,
    /// Used for `https` URLs; its ALPN list decides h1 versus h2.
    pub tls: Option<Arc<rustls::ClientConfig>>,
    /// Speak h2 on plain `http` connections without an upgrade.
    pub http2_prior_knowledge: bool,
    pub pool_idle_timeout: Option<Duration>,
}

/// An established connection as hyper sees it.
struct ConnIo {
    io: TokioIo<Box<dyn Duplex>>,
    negotiated_h2: bool,
}

impl Connection for ConnIo {
    fn connected(&self) -> Connected {
        let connected = Connected::new();
        if self.negotiated_h2 {
            connected.negotiated_h2()
        } else {
            connected
        }
    }
}

impl Read for ConnIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl Write for ConnIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

/// Dials through io-net: phase 1 judges the hostname before any DNS or socket, phase 2 every
/// resolved address (`velyr_io_net::dial`).
#[derive(Clone)]
struct PermissionedConnector {
    permissions: Arc<CapabilitySet>,
    resolver: Resolver,
    tls: Option<TlsConnector>,
}

impl PermissionedConnector {
    async fn connect(self, uri: Uri) -> Result<ConnIo, VelyrError> {
        let host = uri
            .host()
            .ok_or_else(|| VelyrError::invalid_data("request URL has no host"))?;
        let https = uri.scheme_str() == Some("https");
        let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
        if let Err(err) = self
            .permissions
            .check_connect_host(host, port)
            .into_result()
        {
            tracing::warn!(target: "velyr.permission.denied", "net-connect denied for a hostname");
            return Err(err);
        }
        let tcp = dial(host, port, &self.permissions, &self.resolver).await?;
        if !https {
            return Ok(ConnIo {
                io: TokioIo::new(Box::new(tcp)),
                negotiated_h2: false,
            });
        }
        let connector = self
            .tls
            .ok_or_else(|| VelyrError::invalid_data("https needs a TLS client configuration"))?;
        let bare = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let name = ServerName::try_from(bare)
            .map_err(|_| VelyrError::invalid_data("host is not a valid TLS server name"))?;
        let tls = connector
            .connect(name, tcp)
            .await
            .map_err(|err| VelyrError::new(ErrorCode::Tls, err.to_string()))?;
        let negotiated_h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2".as_slice());
        Ok(ConnIo {
            io: TokioIo::new(Box::new(tls)),
            negotiated_h2,
        })
    }
}

impl Service<Uri> for PermissionedConnector {
    type Response = ConnIo;
    type Error = VelyrError;
    type Future = Pin<Box<dyn Future<Output = Result<ConnIo, VelyrError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), VelyrError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        Box::pin(self.clone().connect(uri))
    }
}

pub struct HttpClient {
    inner: Client<PermissionedConnector, BodyReceiver>,
    limits: Limits,
}

/// The `VelyrError` a connector failed with, when one is in the chain; otherwise a network error.
fn client_error(err: hyper_util::client::legacy::Error) -> VelyrError {
    let mut source: Option<&(dyn Error + 'static)> = Some(&err);
    while let Some(current) = source {
        if let Some(velyr) = current.downcast_ref::<VelyrError>() {
            return VelyrError::new(velyr.code(), velyr.message());
        }
        source = current.source();
    }
    VelyrError::new(ErrorCode::Network, err.to_string())
}

fn build_request(parts: HttpRequestParts) -> Result<Request<BodyReceiver>, VelyrError> {
    let invalid = |what: &str| VelyrError::invalid_data(format!("invalid request {what}"));
    let method = Method::from_bytes(parts.method.as_bytes()).map_err(|_| invalid("method"))?;
    let uri: Uri = parts.uri.parse().map_err(|_| invalid("URL"))?;
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in parts.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid("header name"))?;
        let value = HeaderValue::from_str(&value).map_err(|_| invalid("header value"))?;
        request = request.header(name, value);
    }
    request.body(parts.body).map_err(|_| invalid("message"))
}

impl HttpClient {
    pub fn new(
        permissions: Arc<CapabilitySet>,
        resolver: Resolver,
        options: ClientOptions,
    ) -> Self {
        let connector = PermissionedConnector {
            permissions,
            resolver,
            tls: options.tls.map(TlsConnector::from),
        };
        let inner = Client::builder(TokioExecutor::new())
            .timer(TokioTimer::new())
            .pool_timer(TokioTimer::new())
            .pool_idle_timeout(options.pool_idle_timeout.unwrap_or(DEFAULT_POOL_IDLE))
            .http2_only(options.http2_prior_knowledge)
            .http1_max_buf_size(options.limits.max_header_bytes.max(8192))
            .build(connector);
        Self {
            inner,
            limits: options.limits,
        }
    }

    /// Sends `request` and returns once the response head arrived (within `header_timeout`);
    /// the body keeps streaming through the returned channel.
    pub async fn request(
        &self,
        request: HttpRequestParts,
    ) -> Result<HttpResponseParts, VelyrError> {
        let request = build_request(request)?;
        let response =
            tokio::time::timeout(self.limits.header_timeout, self.inner.request(request))
                .await
                .map_err(|_| VelyrError::timeout("no response head before the header timeout"))?
                .map_err(client_error)?;
        let (head, incoming) = response.into_parts();
        let (tx, rx) = body_channel();
        tokio::spawn(pump(incoming, tx, self.limits.clone()));
        Ok(HttpResponseParts {
            status: head.status.as_u16(),
            headers: head
                .headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.to_string(),
                        String::from_utf8_lossy(value.as_bytes()).into_owned(),
                    )
                })
                .collect(),
            body: rx,
            upgrade: None,
        })
    }
}

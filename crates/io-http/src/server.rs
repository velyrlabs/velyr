use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::{Buf, Bytes};
use http::{HeaderName, HeaderValue, Request, Response, StatusCode, header};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use velyr_io_net::Duplex;
use velyr_runtime_errors::{ErrorCode, VelyrError};

use crate::body::pump;
use crate::{BodyReceiver, HttpRequestParts, HttpResponseParts, Limits, body_channel};

pub type HandlerFuture =
    Pin<Box<dyn Future<Output = Result<HttpResponseParts, VelyrError>> + Send>>;

/// Turns a request into a response; an `Err` becomes a 500.
pub type Handler = Arc<dyn Fn(HttpRequestParts) -> HandlerFuture + Send + Sync>;

/// Bytes already read from `inner` (to detect the protocol under a timeout), replayed first.
struct Prefixed<IO> {
    prefix: Bytes,
    inner: IO,
}

impl<IO: AsyncRead + Unpin> AsyncRead for Prefixed<IO> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.prefix.has_remaining() {
            let n = self.prefix.remaining().min(buf.remaining());
            buf.put_slice(&self.prefix[..n]);
            self.prefix.advance(n);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for Prefixed<IO> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

const NONE: u64 = u64::MAX;
/// The h2 client preface (24 bytes) plus one frame header (9 bytes): the least a peer sends once
/// its SETTINGS frame is in, i.e. once the h2 handshake is done.
const H2_HANDSHAKE_BYTES: u64 = 24 + 9;

/// What the IO wrapper saw on one connection; the watcher in [`serve_connection`] reads it.
struct Activity {
    started: Instant,
    last_io: AtomicU64,
    /// When the first byte of a head that no handler has seen yet arrived (h1 slow-head guard).
    head_since: AtomicU64,
    bytes_read: AtomicU64,
    inflight: AtomicUsize,
}

enum Verdict {
    Keep,
    /// Nothing happening: finish gracefully.
    Idle,
    /// Head or h2 handshake took too long: drop the connection.
    Expired,
}

impl Activity {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            last_io: AtomicU64::new(0),
            head_since: AtomicU64::new(NONE),
            bytes_read: AtomicU64::new(0),
            inflight: AtomicUsize::new(0),
        }
    }

    fn now(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(NONE - 1)
    }

    fn touch(&self) {
        self.last_io.store(self.now(), Ordering::Relaxed);
    }

    fn read(&self, bytes: usize) {
        self.touch();
        self.bytes_read.fetch_add(bytes as u64, Ordering::Relaxed);
        if self.inflight.load(Ordering::Relaxed) == 0 {
            let _ = self.head_since.compare_exchange(
                NONE,
                self.now(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
    }

    fn verdict(&self, h2: bool, limits: &Limits) -> Verdict {
        let now = self.now();
        let over = |since: u64, limit: Duration| {
            u128::from(now.saturating_sub(since)) >= limit.as_millis()
        };
        if h2 {
            let shaking = self.bytes_read.load(Ordering::Relaxed) < H2_HANDSHAKE_BYTES;
            return if shaking && over(0, limits.header_timeout) {
                Verdict::Expired
            } else {
                Verdict::Keep
            };
        }
        if self.inflight.load(Ordering::Relaxed) > 0 {
            return Verdict::Keep;
        }
        match self.head_since.load(Ordering::Relaxed) {
            NONE if over(self.last_io.load(Ordering::Relaxed), limits.idle_timeout) => {
                Verdict::Idle
            }
            NONE => Verdict::Keep,
            since if over(since, limits.header_timeout) => Verdict::Expired,
            _ => Verdict::Keep,
        }
    }
}

/// Counts a request as in flight until dropped.
struct InFlight(Arc<Activity>);

impl InFlight {
    fn begin(activity: &Arc<Activity>) -> Self {
        activity.inflight.fetch_add(1, Ordering::Relaxed);
        activity.head_since.store(NONE, Ordering::Relaxed);
        Self(activity.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::Relaxed);
        self.0.touch();
    }
}

/// Reports every read and write to the connection's [`Activity`].
struct Watched<IO> {
    inner: IO,
    activity: Arc<Activity>,
}

impl<IO: AsyncRead + Unpin> AsyncRead for Watched<IO> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = poll {
            let n = buf.filled().len() - before;
            if n > 0 {
                self.activity.read(n);
            }
        }
        poll
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for Watched<IO> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(_)) = poll {
            self.activity.touch();
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

fn status_only(status: StatusCode) -> Response<BodyReceiver> {
    let mut response = Response::new(BodyReceiver::empty());
    *response.status_mut() = status;
    response
}

fn declared_length(request: &Request<Incoming>) -> Option<u64> {
    request
        .headers()
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn response_from(parts: HttpResponseParts) -> Response<BodyReceiver> {
    let mut response = Response::new(parts.body);
    *response.status_mut() = StatusCode::from_u16(parts.status).unwrap_or(StatusCode::BAD_GATEWAY);
    for (name, value) in parts.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            response.headers_mut().append(name, value);
        }
    }
    response
}

async fn handle(
    request: Request<Incoming>,
    peer: Option<SocketAddr>,
    limits: Limits,
    handler: Handler,
    activity: Arc<Activity>,
) -> Result<Response<BodyReceiver>, Infallible> {
    let started = std::time::Instant::now();
    let Ok(response) = answer(request, peer, limits, handler, activity).await;
    crate::metrics::record(response.status().as_u16(), started.elapsed());
    Ok(response)
}

async fn answer(
    mut request: Request<Incoming>,
    peer: Option<SocketAddr>,
    limits: Limits,
    handler: Handler,
    activity: Arc<Activity>,
) -> Result<Response<BodyReceiver>, Infallible> {
    let _in_flight = InFlight::begin(&activity);
    if declared_length(&request).is_some_and(|len| len > limits.max_body_bytes) {
        return Ok(status_only(StatusCode::PAYLOAD_TOO_LARGE));
    }
    let on_upgrade = hyper::upgrade::on(&mut request);
    let (head, incoming) = request.into_parts();
    let (tx, rx) = body_channel();
    tokio::spawn(pump(incoming, tx, limits));
    let parts = HttpRequestParts {
        remote_addr: peer,
        method: head.method.to_string(),
        uri: head
            .uri
            .path_and_query()
            .map_or_else(|| "/".to_owned(), ToString::to_string),
        version: format!("{:?}", head.version),
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
    };
    Ok(match handler(parts).await {
        Ok(mut parts) => {
            if let Some(callback) = parts.upgrade.take().filter(|_| parts.status == 101) {
                tokio::spawn(async move {
                    if let Ok(upgraded) = on_upgrade.await {
                        callback(Box::new(TokioIo::new(upgraded)));
                    }
                });
            }
            response_from(parts)
        }
        Err(err) => {
            tracing::warn!(code = %err.code(), "http handler failed");
            status_only(StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
}

fn builder(limits: &Limits) -> auto::Builder<TokioExecutor> {
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        // Backstop only: hyper also runs this timer while a keep-alive connection is idle, so it
        // must not undercut `idle_timeout`; the watcher enforces the real head and idle limits.
        .header_read_timeout(limits.header_timeout.max(limits.idle_timeout))
        .max_buf_size(limits.max_header_bytes.max(8192));
    builder
        .http2()
        .timer(TokioTimer::new())
        .max_header_list_size(u32::try_from(limits.max_header_bytes).unwrap_or(u32::MAX))
        .keep_alive_interval(Some(limits.idle_timeout))
        .keep_alive_timeout(limits.idle_timeout);
    builder
}

/// Serves one connection until it closes: h1 or h2, detected from the first bytes. The first read
/// is bounded by `limits.header_timeout`; after that a watcher enforces `header_timeout` on a
/// half-sent h1 head and on the h2 handshake, and `idle_timeout` on an idle h1 keep-alive.
pub async fn serve_connection(
    io: impl Duplex,
    peer: Option<SocketAddr>,
    limits: Limits,
    handler: Handler,
) -> Result<(), VelyrError> {
    serve_until(io, peer, limits, handler, CancellationToken::new()).await
}

/// [`serve_connection`] that, once `stop` is cancelled, finishes the requests in flight and closes.
async fn serve_until(
    mut io: impl Duplex,
    peer: Option<SocketAddr>,
    limits: Limits,
    handler: Handler,
    stop: CancellationToken,
) -> Result<(), VelyrError> {
    let mut first = [0u8; 24];
    let read = tokio::time::timeout(limits.header_timeout, io.read(&mut first))
        .await
        .map_err(|_| VelyrError::timeout("no request received before the header timeout"))?
        .map_err(|err| VelyrError::new(ErrorCode::Network, err.to_string()))?;
    if read == 0 {
        return Ok(());
    }
    let h2 = first[..read].starts_with(b"PRI ");
    let activity = Arc::new(Activity::new());
    let io = Watched {
        inner: Prefixed {
            prefix: Bytes::copy_from_slice(&first[..read]),
            inner: io,
        },
        activity: activity.clone(),
    };
    let service_limits = limits.clone();
    let service_activity = activity.clone();
    let service = service_fn(move |request| {
        handle(
            request,
            peer,
            service_limits.clone(),
            handler.clone(),
            service_activity.clone(),
        )
    });
    let builder = builder(&limits);
    let connection = builder.serve_connection_with_upgrades(TokioIo::new(io), service);
    tokio::pin!(connection);
    let tick = (limits.header_timeout.min(limits.idle_timeout) / 4)
        .clamp(Duration::from_millis(10), Duration::from_secs(1));
    let mut ticker = tokio::time::interval(tick);
    let mut stopping = false;
    loop {
        tokio::select! {
            done = connection.as_mut() => {
                return done.map_err(|err| VelyrError::new(ErrorCode::Network, err.to_string()));
            }
            () = stop.cancelled(), if !stopping => {
                stopping = true;
                connection.as_mut().graceful_shutdown();
            }
            _ = ticker.tick() => match activity.verdict(h2, &limits) {
                Verdict::Keep => {}
                Verdict::Idle if !stopping => {
                    stopping = true;
                    connection.as_mut().graceful_shutdown();
                }
                Verdict::Idle => {}
                Verdict::Expired => {
                    return Err(VelyrError::timeout("peer too slow to finish its head or handshake"));
                }
            },
        }
    }
}

/// Accepts connections until `shutdown` is cancelled, doing the TLS handshake first when `tls`
/// is set (give it `h2` and `http/1.1` as ALPN protocols). Each connection runs in its own task;
/// on shutdown they are left running. See [`serve_listener_graceful`] to drain them.
pub async fn serve_listener(
    listener: tokio::net::TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: Limits,
    handler: Handler,
    shutdown: CancellationToken,
) {
    accept_loop(
        Source::Listener(listener),
        tls,
        limits,
        handler,
        shutdown,
        None,
    )
    .await;
}

/// Like [`serve_listener`], but once `shutdown` is cancelled it stops accepting, tells every
/// connection to finish the requests in flight and close, and waits for them for up to `grace`;
/// connections still open then are aborted.
pub async fn serve_listener_graceful(
    listener: tokio::net::TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: Limits,
    handler: Handler,
    shutdown: CancellationToken,
    grace: Duration,
) {
    accept_loop(
        Source::Listener(listener),
        tls,
        limits,
        handler,
        shutdown,
        Some(grace),
    )
    .await;
}

/// Like [`serve_listener_graceful`] for connections accepted somewhere else (a supervisor's
/// acceptor thread) and handed over through `streams`. Ends on shutdown or once the channel is
/// closed and drained, then waits up to `grace` for the connections in flight.
pub async fn serve_streams_graceful(
    streams: tokio::sync::mpsc::Receiver<std::net::TcpStream>,
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: Limits,
    handler: Handler,
    shutdown: CancellationToken,
    grace: Duration,
) {
    accept_loop(
        Source::Streams(streams),
        tls,
        limits,
        handler,
        shutdown,
        Some(grace),
    )
    .await;
}

/// Where accepted connections come from.
enum Source {
    Listener(tokio::net::TcpListener),
    Streams(tokio::sync::mpsc::Receiver<std::net::TcpStream>),
}

impl Source {
    /// `None` when no connection can ever arrive again.
    async fn accept(&mut self) -> Option<std::io::Result<(tokio::net::TcpStream, SocketAddr)>> {
        match self {
            Self::Listener(listener) => Some(listener.accept().await),
            Self::Streams(streams) => {
                let stream = streams.recv().await?;
                Some((|| {
                    let peer = stream.peer_addr()?;
                    stream.set_nonblocking(true)?;
                    Ok((tokio::net::TcpStream::from_std(stream)?, peer))
                })())
            }
        }
    }
}

async fn accept_loop(
    mut source: Source,
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: Limits,
    handler: Handler,
    shutdown: CancellationToken,
    grace: Option<Duration>,
) {
    let acceptor = tls.map(TlsAcceptor::from);
    let stop = if grace.is_some() {
        shutdown.child_token()
    } else {
        CancellationToken::new()
    };
    let mut tasks = JoinSet::new();
    loop {
        let (tcp, peer) = tokio::select! {
            () = shutdown.cancelled() => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => continue,
            accepted = source.accept() => match accepted {
                None => break,
                Some(Ok(accepted)) => accepted,
                Some(Err(err)) => {
                    tracing::warn!(%err, "accept failed");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
            },
        };
        // One `sockets` unit per connection, however many requests it carries.
        let Ok(unit) = crate::quota::reserve(&limits) else {
            crate::quota::reject(tcp);
            continue;
        };
        let (acceptor, limits, handler, stop) = (
            acceptor.clone(),
            limits.clone(),
            handler.clone(),
            stop.clone(),
        );
        tasks.spawn(async move {
            let _unit = unit;
            let result = match acceptor {
                None => serve_until(tcp, Some(peer), limits, handler, stop).await,
                Some(acceptor) => {
                    let handshake =
                        tokio::time::timeout(limits.header_timeout, acceptor.accept(tcp)).await;
                    match handshake {
                        Ok(Ok(tls)) => serve_until(tls, Some(peer), limits, handler, stop).await,
                        _ => return,
                    }
                }
            };
            if let Err(err) = result {
                tracing::debug!(code = %err.code(), "http connection ended with an error");
            }
        });
    }
    match grace {
        None => tasks.detach_all(),
        Some(grace) => {
            drop(source);
            let drained = async { while tasks.join_next().await.is_some() {} };
            if tokio::time::timeout(grace, drained).await.is_err() {
                tracing::warn!("shutdown grace period over, aborting open connections");
            }
        }
    }
}

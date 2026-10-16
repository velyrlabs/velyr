//! Native socket state behind the resource ids JS holds, and the I/O on it. Everything here is
//! `Send + Sync` because the async halves run as spawned tasks while the table stays on the isolate thread.

use std::any::Any;
use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Shutdown, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};

use socket2::SockRef;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_permissions::CapabilitySet;
use velyr_runtime_resources::Resource;

use crate::Resolver;

/// Upper bound for one read, so a huge JS buffer does not turn into a huge allocation.
pub(crate) const MAX_READ: usize = 64 * 1024;

pub fn net_error(err: io::Error) -> VelyrError {
    VelyrError::new(ErrorCode::Network, err.to_string())
}

/// Any byte stream a connection can run over: plain TCP is handled natively, everything else
/// (TLS) is layered on top of a [`TcpIo`] and boxed here.
pub trait Duplex: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Duplex for T {}

struct StreamIo {
    read: tokio::sync::Mutex<ReadHalf<Box<dyn Duplex>>>,
    write: tokio::sync::Mutex<WriteHalf<Box<dyn Duplex>>>,
    /// The TCP connection under the stream: socket options and addresses come from it.
    base: Arc<ConnInner>,
}

enum Io {
    Tcp(TcpStream),
    Stream(StreamIo),
}

pub struct ConnInner {
    pub target: (String, u16),
    /// Empty until `connect_finish` (or accept, or an upgrade) fills it.
    io: OnceLock<Io>,
    /// Set on a placeholder created by an upgrade: the connection it will run over.
    upgrade_base: Mutex<Option<Arc<ConnInner>>>,
    cancel: CancellationToken,
    read_lock: tokio::sync::Mutex<()>,
    write_lock: tokio::sync::Mutex<()>,
}

impl ConnInner {
    pub fn pending(host: String, port: u16) -> Self {
        Self {
            target: (host, port),
            io: OnceLock::new(),
            upgrade_base: Mutex::new(None),
            cancel: CancellationToken::new(),
            read_lock: tokio::sync::Mutex::new(()),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn accepted(stream: TcpStream) -> Self {
        let inner = Self::pending(String::new(), 0);
        inner.io.set(Io::Tcp(stream)).ok().expect("fresh cell");
        inner
    }

    /// Placeholder that will replace `base` once a layer (TLS) is negotiated over it.
    pub fn upgrading(base: Arc<ConnInner>) -> Self {
        let inner = Self::pending(base.target.0.clone(), base.target.1);
        *inner.upgrade_base.lock().expect("upgrade base") = Some(base);
        inner
    }

    /// The connection an [`upgrading`](Self::upgrading) placeholder was created over; once.
    pub fn take_upgrade_base(&self) -> Result<Arc<ConnInner>, VelyrError> {
        self.upgrade_base
            .lock()
            .expect("upgrade base")
            .take()
            .ok_or_else(VelyrError::bad_resource)
    }

    pub fn cancelled(&self) -> impl Future<Output = ()> + '_ {
        self.cancel.cancelled()
    }

    pub fn fill(&self, stream: TcpStream) -> Result<(), VelyrError> {
        self.io
            .set(Io::Tcp(stream))
            .map_err(|_| VelyrError::bad_resource())
    }

    /// Fills a placeholder with a stream layered over `base`.
    pub fn fill_stream(&self, stream: impl Duplex, base: Arc<ConnInner>) -> Result<(), VelyrError> {
        let (read, write) = tokio::io::split(Box::new(stream) as Box<dyn Duplex>);
        let io = Io::Stream(StreamIo {
            read: tokio::sync::Mutex::new(read),
            write: tokio::sync::Mutex::new(write),
            base,
        });
        self.io.set(io).map_err(|_| VelyrError::bad_resource())
    }

    fn io(&self) -> Result<&Io, VelyrError> {
        self.io.get().ok_or_else(VelyrError::bad_resource)
    }

    /// The TCP socket underneath (itself for plain connections).
    pub fn tcp(&self) -> Result<&TcpStream, VelyrError> {
        match self.io()? {
            Io::Tcp(stream) => Ok(stream),
            Io::Stream(io) => io.base.tcp(),
        }
    }

    /// `AsyncRead + AsyncWrite` over the TCP socket, for layering TLS on it. The caller must be the
    /// only user of the connection from here on.
    pub fn tcp_io(self: &Arc<Self>) -> TcpIo {
        TcpIo(self.clone())
    }

    /// `None` at EOF.
    pub async fn read(&self, len: usize) -> Result<Option<Vec<u8>>, VelyrError> {
        let io = self.io()?;
        let _turn = self.read_lock.lock().await;
        let mut buf = vec![0; len.min(MAX_READ)];
        let n = match io {
            Io::Tcp(stream) => self.read_tcp(stream, &mut buf).await?,
            Io::Stream(io) => {
                let mut half = io.read.lock().await;
                tokio::select! {
                    () = self.cancel.cancelled() => return Err(VelyrError::bad_resource()),
                    read = half.read(&mut buf) => read.map_err(net_error)?,
                }
            }
        };
        buf.truncate(n);
        Ok((n > 0).then_some(buf))
    }

    async fn read_tcp(&self, stream: &TcpStream, buf: &mut [u8]) -> Result<usize, VelyrError> {
        loop {
            tokio::select! {
                () = self.cancel.cancelled() => return Err(VelyrError::bad_resource()),
                ready = stream.readable() => ready.map_err(net_error)?,
            }
            match stream.try_read(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                other => return other.map_err(net_error),
            }
        }
    }

    pub async fn write(&self, data: &[u8]) -> Result<usize, VelyrError> {
        let io = self.io()?;
        let _turn = self.write_lock.lock().await;
        match io {
            Io::Tcp(stream) => self.write_tcp(stream, data).await?,
            Io::Stream(io) => {
                let mut half = io.write.lock().await;
                let write = async {
                    half.write_all(data).await?;
                    half.flush().await
                };
                tokio::select! {
                    () = self.cancel.cancelled() => return Err(VelyrError::bad_resource()),
                    written = write => written.map_err(net_error)?,
                }
            }
        }
        Ok(data.len())
    }

    async fn write_tcp(&self, stream: &TcpStream, data: &[u8]) -> Result<(), VelyrError> {
        let mut written = 0;
        while written < data.len() {
            tokio::select! {
                () = self.cancel.cancelled() => return Err(VelyrError::bad_resource()),
                ready = stream.writable() => ready.map_err(net_error)?,
            }
            match stream.try_write(&data[written..]) {
                Ok(n) => written += n,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(net_error(err)),
            }
        }
        Ok(())
    }

    /// Sends FIN (after the TLS close_notify, when layered); a peer that already went away is not an error.
    pub async fn shutdown_write(&self) -> Result<(), VelyrError> {
        match self.io()? {
            Io::Tcp(stream) => shutdown_tcp(stream).map_err(net_error),
            Io::Stream(io) => io.write.lock().await.shutdown().await.map_err(net_error),
        }
    }

    pub fn set_keepalive(&self, on: bool) -> Result<(), VelyrError> {
        SockRef::from(self.tcp()?)
            .set_keepalive(on)
            .map_err(net_error)
    }
}

fn shutdown_tcp(stream: &TcpStream) -> io::Result<()> {
    match SockRef::from(stream).shutdown(Shutdown::Write) {
        Err(err) if err.kind() != io::ErrorKind::NotConnected => Err(err),
        _ => Ok(()),
    }
}

/// Byte-stream view of a plain TCP connection (see [`ConnInner::tcp_io`]).
pub struct TcpIo(Arc<ConnInner>);

impl TcpIo {
    fn stream(&self) -> io::Result<&TcpStream> {
        self.0.tcp().map_err(io::Error::other)
    }
}

impl AsyncRead for TcpIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let stream = self.stream()?;
        loop {
            std::task::ready!(stream.poll_read_ready(cx))?;
            match stream.try_read(buf.initialize_unfilled()) {
                Ok(n) => {
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(err) => return Poll::Ready(Err(err)),
            }
        }
    }
}

impl AsyncWrite for TcpIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let stream = self.stream()?;
        loop {
            std::task::ready!(stream.poll_write_ready(cx))?;
            match stream.try_write(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                other => return Poll::Ready(other),
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(shutdown_tcp(self.stream()?))
    }
}

pub struct ConnRes(pub Arc<ConnInner>);

impl Resource for ConnRes {
    fn kind(&self) -> &'static str {
        "net.conn"
    }
    fn close(self: Box<Self>) {
        self.0.cancel.cancel();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub(crate) struct ListenerInner {
    pub listener: TcpListener,
    /// Accepted by the async half, waiting for `accept_take` to adopt them into the table.
    pub pending: Mutex<VecDeque<TcpStream>>,
    pub cancel: CancellationToken,
}

pub(crate) struct ListenerRes(pub Arc<ListenerInner>);

impl Resource for ListenerRes {
    fn kind(&self) -> &'static str {
        "net.listener"
    }
    fn close(self: Box<Self>) {
        self.0.cancel.cancel();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn addr_json(addr: SocketAddr) -> serde_json::Value {
    serde_json::json!({ "hostname": addr.ip().to_string(), "port": addr.port() })
}

pub fn conn_info(stream: &TcpStream) -> Result<serde_json::Value, VelyrError> {
    let local = stream.local_addr().map_err(net_error)?;
    let remote = stream.peer_addr().map_err(net_error)?;
    Ok(serde_json::json!({ "local": addr_json(local), "remote": addr_json(remote) }))
}

/// IP literal (optionally `[v6]`) or a name for the resolver.
async fn resolve(
    host: &str,
    port: u16,
    resolver: &Resolver,
) -> Result<Vec<SocketAddr>, VelyrError> {
    let literal = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    resolver(host.to_owned(), port).await.map_err(net_error)
}

/// Phase 2 of the connect policy: only addresses that pass `check_connect_addr` are dialed, and
/// the dialed `SocketAddr` is the one that was checked (no second resolution).
pub async fn dial(
    host: &str,
    port: u16,
    permissions: &CapabilitySet,
    resolver: &Resolver,
) -> Result<TcpStream, VelyrError> {
    let candidates = resolve(host, port, resolver).await?;
    let mut denied = None;
    let mut allowed = Vec::new();
    for addr in candidates {
        match permissions.check_connect_addr(host, addr).into_result() {
            Ok(()) => allowed.push(addr),
            Err(err) => denied = Some(err),
        }
    }
    if allowed.is_empty() {
        tracing::warn!(target: "velyr.permission.denied", "net-connect denied for a resolved address");
        return Err(denied.unwrap_or_else(|| {
            VelyrError::new(ErrorCode::Network, "name resolved to no addresses")
        }));
    }
    let mut last = None;
    for addr in allowed {
        match TcpStream::connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(err) => last = Some(err),
        }
    }
    Err(net_error(last.expect("at least one address was tried")))
}

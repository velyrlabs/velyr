//! The `op_serve_*` ops behind `Velyr.serve`. `listen` binds (after the `net-listen` check) and
//! starts the io-http server on the runtime; each request it receives is parked for `accept` /
//! `take`, becomes a resource, and is answered with `respond` (plus `write`/`end` for a streamed
//! body). The io-http side only ever talks to the isolate through [`channel_handler`].

use std::any::Any;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_io_http::{
    BodyReceiver, BodySender, HttpRequestParts, HttpResponseParts, Incoming, Limits,
    UpgradeCallback, body_channel, channel_handler, serve_listener_graceful,
    serve_streams_graceful,
};
use velyr_io_tls::server_config;
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{CapabilityKind, authorize, op_async};
use velyr_runtime_permissions::Request;
use velyr_runtime_resources::{QuotaClass, Resource, ResourceId};

use crate::ops::{op_context, string_arg, type_error};

mod head;

const LISTEN_BACKLOG: u32 = 1024;
/// Most requests `accept` parks per wakeup, so one `take` serves a burst.
const PARK_MAX: usize = 32;
/// How long `shutdown` waits for requests in flight when the caller gives no `graceMs`.
const DEFAULT_GRACE_MS: u64 = 10_000;

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 11] = [
    op!(op_serve_listen, 1, NetListen),
    op!(op_serve_accept, 1, None),
    op!(op_serve_take, 1, None),
    op!(op_serve_read, 1, None),
    op!(op_serve_respond, 4, None),
    op!(op_serve_write, 3, None),
    op!(op_serve_end, 1, None),
    op!(op_serve_fail, 1, None),
    op!(op_serve_shutdown, 1, None),
    op!(op_serve_close, 1, None),
    op!(op_serve_disconnected, 1, None),
];

static TERMINATION: OnceLock<CancellationToken> = OnceLock::new();

/// Cancelled by the host when the process is asked to stop (SIGINT/SIGTERM): every `Velyr.serve`
/// server then shuts down gracefully, exactly as if its script had called `server.shutdown()`.
pub fn termination_token() -> CancellationToken {
    TERMINATION.get_or_init(CancellationToken::new).clone()
}

struct Server {
    /// Starts the graceful shutdown; closing the resource does the same.
    shutdown: CancellationToken,
    requests: Mutex<mpsc::Receiver<Incoming>>,
    /// Requests `accept` received and `take` has not adopted yet.
    parked: StdMutex<VecDeque<Incoming>>,
}

struct ServerRes(Arc<Server>);

impl Resource for ServerRes {
    fn kind(&self) -> &'static str {
        "http_server"
    }

    fn close(self: Box<Self>) {
        self.0.shutdown.cancel();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

type Reply = oneshot::Sender<Result<HttpResponseParts, VelyrError>>;

/// One request being handled.
struct Req {
    cancel: CancellationToken,
    /// The client left before the response head was sent.
    disconnected: CancellationToken,
    body: Mutex<Option<BodyReceiver>>,
    reply: StdMutex<Option<Reply>>,
    /// Feeds a streamed response body until `end`.
    out: Mutex<Option<BodySender>>,
    /// Taken by a `101` response; see [`attach_upgrade`].
    upgrade: StdMutex<Option<UpgradeCallback>>,
    /// OpenTelemetry server span; ended once the response body is out (`respond` for a buffered
    /// one, `write`/`end` for a stream), or on drop if the request never gets that far.
    #[cfg(feature = "telemetry")]
    span: StdMutex<Option<velyr_runtime_telemetry::ActiveSpan>>,
    /// The span's id for scripts (see `crate::telemetry`); released with the request.
    #[cfg(feature = "telemetry")]
    span_id: Option<u64>,
}

#[cfg(feature = "telemetry")]
impl Drop for Req {
    fn drop(&mut self) {
        if let Some(id) = self.span_id {
            crate::telemetry::release(id);
        }
    }
}

impl Req {
    fn end_span(&self) {
        #[cfg(feature = "telemetry")]
        if let Some(span) = self.span.lock().expect("span slot").take() {
            span.end();
        }
    }
}

struct ReqRes(Arc<Req>);

impl Resource for ReqRes {
    fn kind(&self) -> &'static str {
        "http_request"
    }

    /// Dropping an unanswered reply makes the client see a 500; a response body that was not
    /// ended is cut with an error instead of looking complete.
    fn close(self: Box<Self>) {
        self.0.cancel.cancel();
        self.0.reply.lock().expect("reply slot").take();
        let unfinished = self.0.out.try_lock().ok().and_then(|mut out| out.take());
        if let (Some(tx), Ok(runtime)) = (unfinished, tokio::runtime::Handle::try_current()) {
            runtime.spawn(async move {
                tx.send(Err(VelyrError::cancelled())).await;
            });
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn id_arg(args: &[Value], op: &str) -> Result<ResourceId, HostError> {
    match args.first() {
        Some(Value::Number(id)) => Ok(ResourceId::from_js_number(*id)?),
        _ => Err(type_error(op, 0, "a resource id")),
    }
}

fn server_arg(
    ctx: &mut dyn CallContext,
    args: &[Value],
    op: &str,
) -> Result<Arc<Server>, HostError> {
    let id = id_arg(args, op)?;
    Ok(op_context(ctx)?.resources.get::<ServerRes>(id)?.0.clone())
}

fn req_arg(ctx: &mut dyn CallContext, args: &[Value], op: &str) -> Result<Arc<Req>, HostError> {
    let id = id_arg(args, op)?;
    Ok(op_context(ctx)?.resources.get::<ReqRes>(id)?.0.clone())
}

fn network(err: &std::io::Error) -> VelyrError {
    VelyrError::new(ErrorCode::Network, err.to_string())
}

/// What `Velyr.serve({ inherit: true })` serves in a `velyr serve --workers` isolate, instead of
/// a listener of its own and without a `net-listen` check (the supervisor made it). The host puts
/// it on the isolate with `set_slot`.
pub struct ServeInbox {
    /// The address the supervisor serves, reported as the server's address.
    pub addr: SocketAddr,
    /// Connections the supervisor accepted for this isolate.
    pub streams: Option<mpsc::Receiver<std::net::TcpStream>>,
    /// Without `streams`: bind `addr` with `SO_REUSEPORT` and let the kernel spread connections.
    pub reuse_port: bool,
}

/// The IP a `hostname` listens on: an address as is, a name through the system resolver (IPv4
/// first, so `localhost` is stable). Permission checks apply to the result, not the name.
// shortcut: blocks the isolate thread for one `getaddrinfo`; upgrade with an async listen op.
pub fn resolve_listen_host(host: &str) -> Result<IpAddr, VelyrError> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse() {
        return Ok(ip);
    }
    let addrs: Vec<SocketAddr> = std::net::ToSocketAddrs::to_socket_addrs(&(bare, 0))
        .map_err(|e| VelyrError::new(ErrorCode::Network, format!("cannot resolve `{host}`: {e}")))?
        .collect();
    addrs
        .iter()
        .find(|addr| addr.is_ipv4())
        .or(addrs.first())
        .map(SocketAddr::ip)
        .ok_or_else(|| VelyrError::new(ErrorCode::Network, format!("`{host}` has no address")))
}

fn bind(ip: IpAddr, port: u16, reuse_port: bool) -> Result<TcpListener, VelyrError> {
    let socket = if ip.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .map_err(|e| network(&e))?;
    socket.set_reuseaddr(true).map_err(|e| network(&e))?;
    if reuse_port {
        set_reuse_port(&socket)?;
    }
    socket
        .bind(SocketAddr::new(ip, port))
        .map_err(|e| network(&e))?;
    socket.listen(LISTEN_BACKLOG).map_err(|e| network(&e))
}

/// Lets several isolates bind the same address, the kernel spreading connections among them.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn set_reuse_port(socket: &TcpSocket) -> Result<(), VelyrError> {
    socket.set_reuseport(true).map_err(|e| network(&e))
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn set_reuse_port(_socket: &TcpSocket) -> Result<(), VelyrError> {
    Err(VelyrError::invalid_data(
        "reusePort is not supported on this platform",
    ))
}

enum Source {
    Listener(TcpListener),
    Streams(mpsc::Receiver<std::net::TcpStream>),
}

fn json_value(ctx: &mut dyn CallContext, json: serde_json::Value) -> Value {
    ctx.create_string(json.to_string().as_str().into())
}

/// `op_serve_listen(optionsJson) -> string`: options `{hostname, port, cert?, key?, graceMs?,
/// inherit?}`; returns JSON `{id, hostname, port}` with the bound address. Needs
/// `net-listen`, except with `inherit`, which serves the connections the host hands over (its
/// supervisor already checked the permission for the address).
fn op_serve_listen(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.serve_listen").entered();
    let options: serde_json::Value = serde_json::from_str(&string_arg(args, 0, "op_serve_listen")?)
        .map_err(|_| type_error("op_serve_listen", 0, "a JSON options object"))?;
    let host = options["hostname"]
        .as_str()
        .ok_or_else(|| type_error("op_serve_listen", 0, "options with a hostname"))?
        .to_owned();
    let port = options["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(|| type_error("op_serve_listen", 0, "options with a port (0-65535)"))?;
    let inherit = options["inherit"].as_bool().unwrap_or(false);
    let listen_ip = if inherit {
        None
    } else {
        let ip = resolve_listen_host(&host)?;
        authorize(
            ctx,
            "op_serve_listen",
            Request::NetListen(&ip.to_string(), port),
        )?;
        Some(ip)
    };
    let tls = match (options["cert"].as_str(), options["key"].as_str()) {
        (Some(cert), Some(key)) => Some(Arc::new(server_config(
            cert,
            key,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        )?)),
        (None, None) => None,
        _ => {
            return Err(type_error(
                "op_serve_listen",
                0,
                "both cert and key, or neither",
            ));
        }
    };
    let grace = Duration::from_millis(options["graceMs"].as_u64().unwrap_or(DEFAULT_GRACE_MS));
    let (source, bound) = if inherit {
        let inbox = ctx.isolate_slot::<ServeInbox>().ok_or_else(|| {
            type_error(
                "op_serve_listen",
                0,
                "options without inherit (no connections are handed over here)",
            )
        })?;
        match inbox.streams.take() {
            Some(streams) => (Source::Streams(streams), inbox.addr),
            None if inbox.reuse_port => {
                let listener = bind(inbox.addr.ip(), inbox.addr.port(), true)?;
                (Source::Listener(listener), inbox.addr)
            }
            None => {
                return Err(
                    VelyrError::internal("the handed-over connections are already served").into(),
                );
            }
        }
    } else {
        let ip = listen_ip.expect("resolved above unless inheriting");
        let listener = bind(ip, port, false)?;
        let bound = listener.local_addr().map_err(|e| network(&e))?;
        (Source::Listener(listener), bound)
    };
    let (handler, requests) = channel_handler();
    let server = Arc::new(Server {
        shutdown: CancellationToken::new(),
        requests: Mutex::new(requests),
        parked: StdMutex::new(VecDeque::new()),
    });
    let quotas = op_context(ctx)?.quotas.clone();
    let id = op_context(ctx)?.resources.insert(
        Box::new(ServerRes(server.clone())),
        QuotaClass::Listeners,
        Some(format!("{host}:{port}")),
    )?;
    let limits = Limits {
        quotas: Some(quotas),
        ..Limits::default()
    };
    let shutdown = server.shutdown.clone();
    let (stop, termination) = (shutdown.clone(), termination_token());
    tokio::spawn(async move {
        tokio::select! {
            () = termination.cancelled() => stop.cancel(),
            () = stop.cancelled() => {}
        }
    });
    match source {
        Source::Listener(listener) => {
            tokio::spawn(serve_listener_graceful(
                listener, tls, limits, handler, shutdown, grace,
            ));
        }
        Source::Streams(streams) => {
            tokio::spawn(serve_streams_graceful(
                streams, tls, limits, handler, shutdown, grace,
            ));
        }
    }
    Ok(json_value(
        ctx,
        serde_json::json!({"id": id.to_js_number(), "hostname": bound.ip().to_string(), "port": bound.port()}),
    ))
}

/// `op_serve_accept(id) -> Promise<true | null>`: waits for one request and parks it, plus any
/// already queued (up to `PARK_MAX`); `null` once the server stopped and every connection
/// finished. Pair with `op_serve_take`.
fn op_serve_accept(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.serve_accept").entered();
    let server = server_arg(ctx, args, "op_serve_accept")?;
    op_async(ctx, move |token| async move {
        let mut requests = server.requests.lock().await;
        tokio::select! {
            () = token.cancelled() => Ok(CompletionValue::Null),
            next = requests.recv() => match next {
                Some(incoming) => {
                    let mut parked = server.parked.lock().expect("parked queue");
                    parked.push_back(incoming);
                    while parked.len() < PARK_MAX {
                        let Ok(more) = requests.try_recv() else { break };
                        parked.push_back(more);
                    }
                    Ok(CompletionValue::Bool(true))
                }
                None => Ok(CompletionValue::Null),
            },
        }
    })
}

/// `op_serve_take(serverId) -> Array`: adopts every parked request as a resource (a `requests`
/// unit, apart from the `sockets` unit the connection holds) and returns one head per request,
/// laid out as documented in `head.rs`. Over quota the client gets a 503 and the head is `[null]`.
fn op_serve_take(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let server = server_arg(ctx, args, "op_serve_take")?;
    let batch: Vec<Incoming> = server.parked.lock().expect("parked queue").drain(..).collect();
    if batch.is_empty() {
        return Err(VelyrError::new(ErrorCode::InvalidData, "no request is waiting").into());
    }
    let mut heads = Vec::with_capacity(batch.len());
    for incoming in batch {
        heads.push(adopt(ctx, incoming)?);
    }
    Ok(ctx.create_array(&heads))
}

fn adopt(ctx: &mut dyn CallContext, incoming: Incoming) -> Result<Value, HostError> {
    let Incoming {
        request,
        reply,
        disconnected,
    } = incoming;
    let HttpRequestParts {
        remote_addr,
        method,
        uri,
        version,
        headers,
        body,
    } = request;
    #[cfg(feature = "telemetry")]
    let span = velyr_runtime_telemetry::server_span(&method, &uri, &headers);
    #[cfg(feature = "telemetry")]
    let span_id = span.clone().and_then(crate::telemetry::register);
    let req = Arc::new(Req {
        cancel: CancellationToken::new(),
        disconnected,
        body: Mutex::new(Some(body)),
        reply: StdMutex::new(None),
        out: Mutex::new(None),
        upgrade: StdMutex::new(None),
        #[cfg(feature = "telemetry")]
        span: StdMutex::new(span),
        #[cfg(feature = "telemetry")]
        span_id,
    });
    let id = match op_context(ctx)?.resources.insert(
        Box::new(ReqRes(req.clone())),
        QuotaClass::Requests,
        None,
    ) {
        Ok(id) => id,
        Err(_) => {
            let _ = reply.send(Ok(HttpResponseParts {
                status: 503,
                headers: vec![("retry-after".into(), "1".into())],
                body: BodyReceiver::empty(),
                upgrade: None,
            }));
            return Ok(head::to_value(ctx, &[head::Slot::Null]));
        }
    };
    *req.reply.lock().expect("reply slot") = Some(reply);
    #[cfg(feature = "telemetry")]
    let span_id = req.span_id;
    #[cfg(not(feature = "telemetry"))]
    let span_id: Option<u64> = None;
    let head = head::Head {
        id: id.to_js_number(),
        method: &method,
        uri: &uri,
        version: &version,
        remote: remote_addr.map(|addr| (addr.ip().to_string(), addr.port())),
        span_id,
        headers: &headers,
    };
    Ok(head::to_value(ctx, &head.slots()))
}

/// `op_serve_read(id) -> Promise<ArrayBuffer | null>`: next piece of the request body; `null` at
/// the end. A body over the limits is an error.
fn op_serve_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_serve_read")?;
    op_async(ctx, move |token| async move {
        let mut guard = req.body.lock().await;
        let body = guard.as_mut().ok_or_else(VelyrError::bad_resource)?;
        let chunk = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            () = req.cancel.cancelled() => return Err(VelyrError::cancelled()),
            chunk = body.recv() => chunk,
        };
        match chunk {
            None => Ok(CompletionValue::Null),
            Some(chunk) => Ok(CompletionValue::Bytes(chunk?.to_vec())),
        }
    })
}

/// `op_serve_respond(id, status, headers, body)`: `headers` is `name\0value\0...` (see `head.rs`);
/// `body` is `null`, bytes, a string (sent as UTF-8), or `true` for a body fed through
/// `op_serve_write` / `op_serve_end`. A client that already left is an error.
fn op_serve_respond(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.serve_respond").entered();
    let req = req_arg(ctx, args, "op_serve_respond")?;
    let status = match args.get(1) {
        Some(Value::Number(status))
            if (100.0..=599.0).contains(status) && status.fract() == 0.0 =>
        {
            *status as u16
        }
        _ => return Err(type_error("op_serve_respond", 1, "a status code (100-599)")),
    };
    let headers = head::parse_header_blob(&string_arg(args, 2, "op_serve_respond")?)?;
    let (body, out) = match args.get(3) {
        Some(Value::Bool(true)) => {
            let (tx, rx) = body_channel();
            (rx, Some(tx))
        }
        Some(value @ Value::Object(_)) => {
            let bytes = ctx
                .read_bytes(value)
                .ok_or_else(|| type_error("op_serve_respond", 3, "bytes, true or null"))?;
            (BodyReceiver::once(bytes), None)
        }
        Some(Value::String(text)) => {
            let mut bytes = Vec::new();
            ctx.write_string_utf8(text, &mut bytes);
            (BodyReceiver::once(bytes), None)
        }
        _ => (BodyReceiver::empty(), None),
    };
    let reply = req
        .reply
        .lock()
        .expect("reply slot")
        .take()
        .ok_or_else(|| VelyrError::internal("the request was already answered"))?;
    #[cfg(feature = "telemetry")]
    if let Some(span) = req.span.lock().expect("span slot").as_ref() {
        span.record_status(status, 500);
    }
    if out.is_none() {
        req.end_span();
    }
    // Set before the reply: the caller starts writing as soon as this returns.
    *req.out
        .try_lock()
        .map_err(|_| VelyrError::internal("response body slot is busy"))? = out;
    let upgrade = if status == 101 {
        req.upgrade.lock().expect("upgrade slot").take()
    } else {
        None
    };
    reply
        .send(Ok(HttpResponseParts {
            status,
            headers,
            body,
            upgrade,
        }))
        .map_err(|_| VelyrError::new(ErrorCode::Network, "the client closed the connection"))?;
    Ok(Value::Undefined)
}

/// Lets `callback` take over the connection of request `request_id` (a number from the ops
/// above) once the handler answers it with a `101` response; without one the connection stays
/// plain HTTP. For protocol layers such as WebSocket.
pub fn attach_upgrade(
    ctx: &mut dyn CallContext,
    request_id: f64,
    callback: UpgradeCallback,
) -> Result<(), HostError> {
    let id = ResourceId::from_js_number(request_id)?;
    let req = op_context(ctx)?.resources.get::<ReqRes>(id)?.0.clone();
    *req.upgrade.lock().expect("upgrade slot") = Some(callback);
    Ok(())
}

/// `op_serve_write(id, data, end) -> Promise<undefined>`: next piece of a streamed response body;
/// waits while the connection is not ready for more. With `end` set the body is complete after
/// this piece (what `op_serve_end` does, without a second round trip).
fn op_serve_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_serve_write")?;
    let data = args
        .get(1)
        .and_then(|value| ctx.read_bytes(value))
        .ok_or_else(|| type_error("op_serve_write", 1, "an ArrayBuffer or ArrayBufferView"))?;
    let end = matches!(args.get(2), Some(Value::Bool(true)));
    op_async(ctx, move |token| async move {
        let mut guard = req.out.lock().await;
        let tx = guard.as_ref().ok_or_else(VelyrError::bad_resource)?;
        let sent = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            () = req.cancel.cancelled() => return Err(VelyrError::cancelled()),
            sent = tx.send(Ok(data.into())) => sent,
        };
        if !sent {
            return Err(VelyrError::new(
                ErrorCode::Network,
                "the client closed the connection",
            ));
        }
        if end {
            guard.take();
            req.end_span();
        }
        Ok(CompletionValue::Undefined)
    })
}

/// `op_serve_end(id) -> Promise<undefined>`: the streamed response body is complete.
fn op_serve_end(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_serve_end")?;
    op_async(ctx, move |_token| async move {
        req.out.lock().await.take();
        req.end_span();
        Ok(CompletionValue::Undefined)
    })
}

/// `op_serve_fail(id)`: answers an unanswered request with a 500.
fn op_serve_fail(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_serve_fail")?;
    if let Some(reply) = req.reply.lock().expect("reply slot").take() {
        let _ = reply.send(Err(VelyrError::internal("the request handler failed")));
    }
    Ok(Value::Undefined)
}

/// `op_serve_shutdown(serverId)`: stop accepting and let requests in flight finish.
fn op_serve_shutdown(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    server_arg(ctx, args, "op_serve_shutdown")?
        .shutdown
        .cancel();
    Ok(Value::Undefined)
}

/// `op_serve_close(id)`: releases a server or a request.
fn op_serve_close(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = id_arg(args, "op_serve_close")?;
    op_context(ctx)?.resources.close(id)?;
    Ok(Value::Undefined)
}

/// `op_serve_disconnected(id) -> Promise<boolean>`: `true` once the client left before the
/// response head was sent; `false` when the request ended first (its resource closed).
fn op_serve_disconnected(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_serve_disconnected")?;
    op_async(ctx, move |token| async move {
        tokio::select! {
            () = req.disconnected.cancelled() => Ok(CompletionValue::Bool(true)),
            () = req.cancel.cancelled() => Ok(CompletionValue::Bool(false)),
            () = token.cancelled() => Ok(CompletionValue::Bool(false)),
        }
    })
}

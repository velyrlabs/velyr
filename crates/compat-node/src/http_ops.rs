//! Host ops behind the `node:http` / `node:https` server: `op_node_http_*`. `listen` binds (after
//! the `net-listen` check) and starts the io-http server on the runtime; each request it hands over
//! is parked for `accept` / `take`, becomes a resource, and is answered with `respond` (plus `write`
//! / `end` for a streamed body). The upgraded connection of a `101` response is read and written
//! through `up_*`. io-http talks to the isolate only through `channel_handler`, so hyper never
//! runs JS and a slow script only pushes back on the connections.

mod head;

use std::any::Any;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_io_http::{
    BodyReceiver, BodySender, HttpRequestParts, HttpResponseParts, Incoming, Limits,
    UpgradeCallback, body_channel, channel_handler, serve_listener_graceful,
};
use velyr_io_net::Duplex;

use crate::http_upgrade_ops::{Upgraded, http_upgrade_host_functions};
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{OpContext, authorize, op_async};
use velyr_runtime_permissions::Request;
use velyr_runtime_resources::{QuotaClass, Resource, ResourceId};

const LISTEN_BACKLOG: u32 = 1024;
const GRACE: Duration = Duration::from_secs(5);
/// Requests one `accept` parks at most, so a burst costs one await and one `take`.
const PARK_MAX: usize = 32;

macro_rules! op {
    ($name:ident, $arity:expr) => {
        HostFunctionDef {
            name: stringify!($name),
            arity: $arity,
            callback: $name,
        }
    };
}

pub(crate) fn http_host_functions() -> Vec<HostFunctionDef> {
    vec![
        op!(op_node_http_listen, 1),
        op!(op_node_http_accept, 1),
        op!(op_node_http_take, 1),
        op!(op_node_http_read, 1),
        op!(op_node_http_respond, 4),
        op!(op_node_http_write, 3),
        op!(op_node_http_end, 1),
        op!(op_node_http_fail, 1),
        op!(op_node_http_shutdown, 1),
        op!(op_node_http_close, 1),
        op!(op_node_http_disconnected, 1),
    ]
    .into_iter()
    .chain(http_upgrade_host_functions())
    .collect()
}

pub(crate) fn op_context(ctx: &mut dyn CallContext) -> Result<&mut OpContext, HostError> {
    ctx.isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))
}

pub(crate) fn type_error(op: &str, index: usize, wanted: &str) -> HostError {
    HostError::TypeError(format!("{op} expects argument {index} to be {wanted}"))
}

pub(crate) fn string_arg(args: &[Value], index: usize, op: &str) -> Result<String, HostError> {
    match args.get(index) {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        _ => Err(type_error(op, index, "a string")),
    }
}

pub(crate) fn network(err: &std::io::Error) -> VelyrError {
    VelyrError::new(ErrorCode::Network, err.to_string())
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
        "node_http_server"
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
pub(crate) struct Req {
    pub(crate) cancel: CancellationToken,
    /// The client left before the response head was sent.
    disconnected: CancellationToken,
    body: Mutex<Option<BodyReceiver>>,
    reply: StdMutex<Option<Reply>>,
    /// Feeds a streamed response body until `end`.
    out: Mutex<Option<BodySender>>,
    /// Taken by a `101` response.
    pub(crate) upgrade: StdMutex<Option<UpgradeCallback>>,
    pub(crate) upgrade_rx: Mutex<Option<oneshot::Receiver<Box<dyn Duplex>>>>,
    pub(crate) upgraded: Upgraded,
}

struct ReqRes(Arc<Req>);

impl Resource for ReqRes {
    fn kind(&self) -> &'static str {
        "node_http_request"
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

pub(crate) fn req_arg(
    ctx: &mut dyn CallContext,
    args: &[Value],
    op: &str,
) -> Result<Arc<Req>, HostError> {
    let id = id_arg(args, op)?;
    Ok(op_context(ctx)?.resources.get::<ReqRes>(id)?.0.clone())
}

fn json_value(ctx: &mut dyn CallContext, json: serde_json::Value) -> Value {
    ctx.create_string(json.to_string().as_str().into())
}

fn bind(ip: IpAddr, port: u16) -> Result<TcpListener, VelyrError> {
    let socket = if ip.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .map_err(|e| network(&e))?;
    socket.set_reuseaddr(true).map_err(|e| network(&e))?;
    socket
        .bind(SocketAddr::new(ip, port))
        .map_err(|e| network(&e))?;
    socket.listen(LISTEN_BACKLOG).map_err(|e| network(&e))
}

/// `op_node_http_listen(optionsJson) -> string`: options `{hostname, port, cert?, key?}` with an IP
/// `hostname` (the JS side resolves names); returns JSON `{id, hostname, port}`. Needs
/// `net-listen`. With `cert` and `key` the server speaks TLS, offering only `http/1.1`.
fn op_node_http_listen(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let options: serde_json::Value =
        serde_json::from_str(&string_arg(args, 0, "op_node_http_listen")?)
            .map_err(|_| type_error("op_node_http_listen", 0, "a JSON options object"))?;
    let host = options["hostname"].as_str().unwrap_or_default().to_owned();
    let ip: IpAddr = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .map_err(|_| type_error("op_node_http_listen", 0, "options with an IP hostname"))?;
    let port = options["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(|| type_error("op_node_http_listen", 0, "options with a port (0-65535)"))?;
    authorize(
        ctx,
        "op_node_http_listen",
        Request::NetListen(&ip.to_string(), port),
    )?;
    let tls = match (options["cert"].as_str(), options["key"].as_str()) {
        (Some(cert), Some(key)) => Some(Arc::new(velyr_io_tls::server_config(
            cert,
            key,
            vec![b"http/1.1".to_vec()],
        )?)),
        (None, None) => None,
        _ => {
            return Err(type_error(
                "op_node_http_listen",
                0,
                "both cert and key, or neither",
            ));
        }
    };
    let listener = bind(ip, port)?;
    let bound = listener.local_addr().map_err(|e| network(&e))?;
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
    tokio::spawn(serve_listener_graceful(
        listener,
        tls,
        limits,
        handler,
        server.shutdown.clone(),
        GRACE,
    ));
    Ok(json_value(
        ctx,
        serde_json::json!({"id": id.to_js_number(), "hostname": bound.ip().to_string(), "port": bound.port()}),
    ))
}

/// `op_node_http_accept(id) -> Promise<true | null>`: waits for one request and parks it; `null`
/// once the server stopped and every connection finished. Pair with `op_node_http_take`.
fn op_node_http_accept(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let server = server_arg(ctx, args, "op_node_http_accept")?;
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

/// `op_node_http_take(serverId) -> Array`: adopts every parked request as a resource (a `requests`
/// unit each) and returns their heads in order, layout in `head.rs`; over quota the client gets a
/// 503 and the head is `[null]`.
fn op_node_http_take(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let server = server_arg(ctx, args, "op_node_http_take")?;
    let parked: Vec<Incoming> = server.parked.lock().expect("parked queue").drain(..).collect();
    if parked.is_empty() {
        return Err(VelyrError::new(ErrorCode::InvalidData, "no request is waiting").into());
    }
    let mut heads = Vec::with_capacity(parked.len());
    for Incoming {
        request,
        reply,
        disconnected,
    } in parked
    {
        let items = adopt(ctx, request, reply, disconnected)?;
        heads.push(ctx.create_array(&items));
    }
    Ok(ctx.create_array(&heads))
}

fn adopt(
    ctx: &mut dyn CallContext,
    mut request: HttpRequestParts,
    reply: Reply,
    disconnected: CancellationToken,
) -> Result<Vec<Value>, HostError> {
    let body = std::mem::replace(&mut request.body, BodyReceiver::empty());
    let req = Arc::new(Req {
        cancel: CancellationToken::new(),
        disconnected,
        body: Mutex::new(Some(body)),
        reply: StdMutex::new(None),
        out: Mutex::new(None),
        upgrade: StdMutex::new(None),
        upgrade_rx: Mutex::new(None),
        upgraded: Upgraded::default(),
    });
    let inserted = op_context(ctx)?.resources.insert(
        Box::new(ReqRes(req.clone())),
        QuotaClass::Requests,
        None,
    );
    let Ok(id) = inserted else {
        let _ = reply.send(Ok(HttpResponseParts {
            status: 503,
            headers: vec![("retry-after".into(), "1".into())],
            body: BodyReceiver::empty(),
            upgrade: None,
        }));
        return Ok(vec![Value::Null]);
    };
    *req.reply.lock().expect("reply slot") = Some(reply);
    Ok(head::head_items(id.to_js_number(), &request))
}

/// `op_node_http_read(id) -> Promise<ArrayBuffer | null>`: next piece of the request body; `null`
/// at the end. A body over the limits is an error.
fn op_node_http_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_node_http_read")?;
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

/// `op_node_http_respond(id, status, headerBlob, body)`: headers as `name\0value\0...`; `body` is `null`, bytes, or `true` for a
/// body fed through `op_node_http_write` / `op_node_http_end`. A client that already left is an
/// error.
fn op_node_http_respond(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let op = "op_node_http_respond";
    let req = req_arg(ctx, args, op)?;
    let status = match args.get(1) {
        Some(Value::Number(status))
            if (100.0..=599.0).contains(status) && status.fract() == 0.0 =>
        {
            *status as u16
        }
        _ => return Err(type_error(op, 1, "a status code (100-599)")),
    };
    let headers = head::parse_header_blob(&string_arg(args, 2, op)?)?;
    let (body, out) = match args.get(3) {
        Some(Value::Bool(true)) => {
            let (tx, rx) = body_channel();
            (rx, Some(tx))
        }
        Some(value @ Value::Object(_)) => {
            let bytes = ctx
                .read_bytes(value)
                .ok_or_else(|| type_error(op, 3, "bytes, true or null"))?;
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

/// `op_node_http_write(id, data, end) -> Promise<undefined>`: next piece of a streamed response
/// body; waits while the connection is not ready for more. With `end` set the body is complete
/// after this piece.
fn op_node_http_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_node_http_write")?;
    let data = args
        .get(1)
        .and_then(|value| ctx.read_bytes(value))
        .ok_or_else(|| type_error("op_node_http_write", 1, "an ArrayBuffer or view"))?;
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
        }
        Ok(CompletionValue::Undefined)
    })
}

/// `op_node_http_end(id) -> Promise<undefined>`: the streamed response body is complete.
fn op_node_http_end(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_node_http_end")?;
    op_async(ctx, move |_token| async move {
        req.out.lock().await.take();
        Ok(CompletionValue::Undefined)
    })
}

/// `op_node_http_fail(id)`: answers an unanswered request with a 500.
fn op_node_http_fail(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_node_http_fail")?;
    if let Some(reply) = req.reply.lock().expect("reply slot").take() {
        let _ = reply.send(Err(VelyrError::internal("the request handler failed")));
    }
    Ok(Value::Undefined)
}

/// `op_node_http_shutdown(serverId)`: stop accepting and let requests in flight finish.
fn op_node_http_shutdown(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    server_arg(ctx, args, "op_node_http_shutdown")?
        .shutdown
        .cancel();
    Ok(Value::Undefined)
}

/// `op_node_http_close(id)`: releases a server or a request.
fn op_node_http_close(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = id_arg(args, "op_node_http_close")?;
    op_context(ctx)?.resources.close(id)?;
    Ok(Value::Undefined)
}

/// `op_node_http_disconnected(id) -> Promise<boolean>`: `true` once the client left before the
/// response head was sent; `false` when the request ended first (its resource closed).
fn op_node_http_disconnected(
    ctx: &mut dyn CallContext,
    args: &[Value],
) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_node_http_disconnected")?;
    op_async(ctx, move |token| async move {
        tokio::select! {
            () = req.disconnected.cancelled() => Ok(CompletionValue::Bool(true)),
            () = req.cancel.cancelled() => Ok(CompletionValue::Bool(false)),
            () = token.cancelled() => Ok(CompletionValue::Bool(false)),
        }
    })
}

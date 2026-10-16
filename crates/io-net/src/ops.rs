use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use tokio::net::TcpSocket;
use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{CapabilityKind, OpContext, authorize, op_async};
use velyr_runtime_permissions::Request;
use velyr_runtime_resources::{QuotaClass, ResourceId};

use crate::conn::{
    ConnInner, ConnRes, ListenerInner, ListenerRes, addr_json, conn_info, dial, net_error,
};
use crate::{NetState, Resolver};

const LISTEN_BACKLOG: u32 = 1024;

macro_rules! op {
    ($name:ident, $arity:expr, $kind:ident) => {
        (
            HostFunctionDef {
                name: stringify!($name),
                arity: $arity,
                callback: $name,
            },
            CapabilityKind::$kind,
        )
    };
}

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 11] = [
    op!(op_net_connect_begin, 2, NetConnect),
    op!(op_net_connect_finish, 1, None),
    op!(op_net_listen, 2, NetListen),
    op!(op_net_accept, 1, None),
    op!(op_net_accept_take, 1, None),
    op!(op_net_read, 2, None),
    op!(op_net_write, 2, None),
    op!(op_net_shutdown, 1, None),
    op!(op_net_close, 1, None),
    op!(op_net_set_nodelay, 2, None),
    op!(op_net_set_keepalive, 2, None),
];

fn type_error(op: &str, index: usize, what: &str) -> HostError {
    HostError::TypeError(format!("{op} expects argument {index} to be {what}"))
}

fn string_arg(args: &[Value], index: usize, op: &str) -> Result<String, HostError> {
    match args.get(index) {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        _ => Err(type_error(op, index, "a string")),
    }
}

fn bool_arg(args: &[Value], index: usize, op: &str) -> Result<bool, HostError> {
    match args.get(index) {
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(type_error(op, index, "a boolean")),
    }
}

fn number_arg(args: &[Value], index: usize, op: &str) -> Result<f64, HostError> {
    match args.get(index) {
        Some(Value::Number(value)) => Ok(*value),
        _ => Err(type_error(op, index, "a number")),
    }
}

fn port_arg(args: &[Value], index: usize, op: &str) -> Result<u16, HostError> {
    let port = number_arg(args, index, op)?;
    if port.fract() != 0.0 || !(0.0..=65535.0).contains(&port) {
        return Err(type_error(op, index, "a port number (0-65535)"));
    }
    Ok(port as u16)
}

fn id_arg(args: &[Value], op: &str) -> Result<ResourceId, HostError> {
    Ok(ResourceId::from_js_number(number_arg(args, 0, op)?)?)
}

fn op_context(ctx: &mut dyn CallContext) -> Result<&mut OpContext, HostError> {
    ctx.isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))
}

fn conn_arg(
    ctx: &mut dyn CallContext,
    args: &[Value],
    op: &str,
) -> Result<Arc<ConnInner>, HostError> {
    let id = id_arg(args, op)?;
    Ok(op_context(ctx)?.resources.get::<ConnRes>(id)?.0.clone())
}

fn listener_arg(
    ctx: &mut dyn CallContext,
    args: &[Value],
    op: &str,
) -> Result<Arc<ListenerInner>, HostError> {
    let id = id_arg(args, op)?;
    Ok(op_context(ctx)?.resources.get::<ListenerRes>(id)?.0.clone())
}

fn json_value(ctx: &mut dyn CallContext, json: serde_json::Value) -> Value {
    ctx.create_string(json.to_string().as_str().into())
}

fn strip_brackets(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
}

/// `op_net_connect_begin(hostname, port) -> id`: phase 1 (hostname, before DNS), then a
/// placeholder socket that reserves one `sockets` unit. The caller must `close` it if
/// `connect_finish` fails.
fn op_net_connect_begin(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.net_connect_begin").entered();
    let host = string_arg(args, 0, "op_net_connect_begin")?;
    let port = port_arg(args, 1, "op_net_connect_begin")?;
    let id = begin_connect(ctx, "op_net_connect_begin", host, port)?;
    Ok(Value::Number(id.to_js_number()))
}

/// Phase 1 of a connect for `op` (also used by layered protocols): authorizes `host:port`, then
/// inserts a placeholder socket that reserves one `sockets` unit.
pub fn begin_connect(
    ctx: &mut dyn CallContext,
    op: &'static str,
    host: String,
    port: u16,
) -> Result<ResourceId, HostError> {
    authorize(ctx, op, Request::NetConnect(&host, port))?;
    let capability = format!("{host}:{port}");
    let inner = Arc::new(ConnInner::pending(host, port));
    Ok(op_context(ctx)?.resources.insert(
        Box::new(ConnRes(inner)),
        QuotaClass::Sockets,
        Some(capability),
    )?)
}

/// The socket resource `id` (a placeholder included).
pub fn conn_by_id(ctx: &mut dyn CallContext, id: ResourceId) -> Result<Arc<ConnInner>, HostError> {
    Ok(op_context(ctx)?.resources.get::<ConnRes>(id)?.0.clone())
}

/// Removes socket `id` from the table without closing it and releases its quota; the caller owns
/// the connection from here on.
pub fn take_conn(ctx: &mut dyn CallContext, id: ResourceId) -> Result<Arc<ConnInner>, HostError> {
    let table = &mut op_context(ctx)?.resources;
    let inner = table.get::<ConnRes>(id)?.0.clone();
    table.take(id)?;
    Ok(inner)
}

/// Inserts a placeholder that will replace `base` (see [`ConnInner::upgrading`]); reserves one
/// `sockets` unit.
pub fn insert_upgrading(
    ctx: &mut dyn CallContext,
    base: Arc<ConnInner>,
) -> Result<ResourceId, HostError> {
    let inner = Arc::new(ConnInner::upgrading(base));
    Ok(op_context(ctx)?
        .resources
        .insert(Box::new(ConnRes(inner)), QuotaClass::Sockets, None)?)
}

/// The resolver installed with the net bootstrap.
pub fn resolver_of(ctx: &mut dyn CallContext) -> Result<Resolver, HostError> {
    Ok(ctx
        .isolate_slot::<NetState>()
        .ok_or_else(|| HostError::TypeError("net ops need an installed NetState".into()))?
        .resolver
        .clone())
}

/// `op_net_connect_finish(id) -> Promise<string>`: resolves, phase 2 per address, dials; JSON
/// `{local, remote}`.
fn op_net_connect_finish(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.net_connect_finish").entered();
    let inner = conn_arg(ctx, args, "op_net_connect_finish")?;
    let permissions = op_context(ctx)?.permissions.clone();
    let resolver = resolver_of(ctx)?;
    op_async(ctx, |token| async move {
        let (host, port) = &inner.target;
        let stream = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            () = inner.cancelled() => return Err(VelyrError::bad_resource()),
            dialed = dial(host, *port, &permissions, &resolver) => dialed?,
        };
        let info = conn_info(&stream)?;
        inner.fill(stream)?;
        Ok(CompletionValue::String(info.to_string()))
    })
}

fn bind_listener(host: &str, port: u16) -> Result<ListenerInner, VelyrError> {
    let ip: IpAddr = strip_brackets(host)
        .parse()
        .map_err(|_| VelyrError::invalid_data("listen hostname must be an IP address"))?;
    let socket = if ip.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .map_err(net_error)?;
    socket.set_reuseaddr(true).map_err(net_error)?;
    socket.bind(SocketAddr::new(ip, port)).map_err(net_error)?;
    Ok(ListenerInner {
        listener: socket.listen(LISTEN_BACKLOG).map_err(net_error)?,
        pending: Mutex::new(VecDeque::new()),
        cancel: CancellationToken::new(),
    })
}

/// `op_net_listen(hostname, port) -> string`: JSON `{id, hostname, port}` with the bound address.
fn op_net_listen(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.net_listen").entered();
    let host = string_arg(args, 0, "op_net_listen")?;
    let port = port_arg(args, 1, "op_net_listen")?;
    authorize(ctx, "op_net_listen", Request::NetListen(&host, port))?;
    let listener = bind_listener(&host, port)?;
    let bound = listener.listener.local_addr().map_err(net_error)?;
    let id = op_context(ctx)?.resources.insert(
        Box::new(ListenerRes(Arc::new(listener))),
        QuotaClass::Listeners,
        Some(format!("{host}:{port}")),
    )?;
    let mut json = addr_json(bound);
    json["id"] = id.to_js_number().into();
    Ok(json_value(ctx, json))
}

/// `op_net_accept(id) -> Promise<true | null>`: waits for one connection and parks it in the
/// listener; `null` means the listener was closed. Pair with `accept_take`.
fn op_net_accept(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.net_accept").entered();
    let listener = listener_arg(ctx, args, "op_net_accept")?;
    op_async(ctx, |token| async move {
        tokio::select! {
            () = token.cancelled() => Ok(CompletionValue::Null),
            () = listener.cancel.cancelled() => Ok(CompletionValue::Null),
            accepted = listener.listener.accept() => {
                let (stream, _) = accepted.map_err(net_error)?;
                listener.pending.lock().expect("pending queue").push_back(stream);
                Ok(CompletionValue::Bool(true))
            }
        }
    })
}

/// `op_net_accept_take(listenerId) -> string`: adopts the oldest parked connection as a socket
/// resource (reserving a `sockets` unit; over quota the connection is dropped); JSON
/// `{id, local, remote}`.
fn op_net_accept_take(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let listener = listener_arg(ctx, args, "op_net_accept_take")?;
    let stream = listener
        .pending
        .lock()
        .expect("pending queue")
        .pop_front()
        .ok_or_else(|| {
            VelyrError::new(ErrorCode::InvalidData, "no accepted connection is waiting")
        })?;
    let mut info = conn_info(&stream)?;
    let id = op_context(ctx)?.resources.insert(
        Box::new(ConnRes(Arc::new(ConnInner::accepted(stream)))),
        QuotaClass::Sockets,
        None,
    )?;
    info["id"] = id.to_js_number().into();
    Ok(json_value(ctx, info))
}

/// `op_net_read(id, length) -> Promise<ArrayBuffer | null>`: at most `length` bytes; `null` at EOF.
fn op_net_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.net_read").entered();
    let conn = conn_arg(ctx, args, "op_net_read")?;
    let len = number_arg(args, 1, "op_net_read")?;
    if len.fract() != 0.0 || len < 1.0 {
        return Err(type_error("op_net_read", 1, "a positive integer"));
    }
    op_async(ctx, |_token| async move {
        Ok(match conn.read(len as usize).await? {
            Some(bytes) => CompletionValue::Bytes(bytes),
            None => CompletionValue::Null,
        })
    })
}

/// `op_net_write(id, data) -> Promise<number>`: writes all of `data` (ArrayBuffer or view).
fn op_net_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.net_write").entered();
    let conn = conn_arg(ctx, args, "op_net_write")?;
    let data = args
        .get(1)
        .and_then(|value| ctx.read_bytes(value))
        .ok_or_else(|| type_error("op_net_write", 1, "an ArrayBuffer or ArrayBufferView"))?;
    op_async(ctx, |_token| async move {
        Ok(CompletionValue::Number(conn.write(&data).await? as f64))
    })
}

/// `op_net_shutdown(id) -> Promise<undefined>`: half-close the write side (FIN; close_notify first on TLS).
fn op_net_shutdown(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let conn = conn_arg(ctx, args, "op_net_shutdown")?;
    op_async(ctx, |_token| async move {
        conn.shutdown_write().await?;
        Ok(CompletionValue::Undefined)
    })
}

/// `op_net_close(id)`: closes a socket or a listener; pending reads/accepts end.
fn op_net_close(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = id_arg(args, "op_net_close")?;
    op_context(ctx)?.resources.close(id)?;
    Ok(Value::Undefined)
}

fn op_net_set_nodelay(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let conn = conn_arg(ctx, args, "op_net_set_nodelay")?;
    let on = bool_arg(args, 1, "op_net_set_nodelay")?;
    conn.tcp()?.set_nodelay(on).map_err(net_error)?;
    Ok(Value::Undefined)
}

fn op_net_set_keepalive(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let conn = conn_arg(ctx, args, "op_net_set_keepalive")?;
    conn.set_keepalive(bool_arg(args, 1, "op_net_set_keepalive")?)?;
    Ok(Value::Undefined)
}

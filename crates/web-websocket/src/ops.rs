//! The `op_ws_*` ops. A connection is one resource; the task in [`crate::socket`] owns the
//! stream and JS talks to it through queues: `send`/`close` push, `next` pulls events.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Uri};
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_tungstenite::{WebSocketStream, client_async_with_config};
use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_io_net::{Duplex, dial, resolver_of};
use velyr_io_tls::client_config;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{CapabilityKind, OpContext, authorize, op_async};
use velyr_runtime_permissions::Request;
use velyr_runtime_resources::{QuotaClass, Resource, ResourceId};

use crate::socket::{EVENT_QUEUE, Event, Link, Outgoing, config, drive};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

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

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 8] = [
    op!(op_ws_connect, 2, NetConnect),
    op!(op_ws_upgrade, 2, None),
    op!(op_ws_next, 1, None),
    op!(op_ws_send, 2, None),
    op!(op_ws_buffered, 1, None),
    op!(op_ws_close, 3, None),
    op!(op_ws_release, 1, None),
    op!(op_ws_accept_key, 1, None),
];

struct Ws {
    outgoing: mpsc::UnboundedSender<Outgoing>,
    events: Mutex<mpsc::Receiver<Event>>,
    buffered: Arc<AtomicU64>,
    cancel: CancellationToken,
}

struct WsRes(Arc<Ws>);

impl Resource for WsRes {
    fn kind(&self) -> &'static str {
        "websocket"
    }

    fn close(self: Box<Self>) {
        self.0.cancel.cancel();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn type_error(op: &str, index: usize, what: &str) -> HostError {
    HostError::TypeError(format!("{op} expects argument {index} to be {what}"))
}

fn string_arg(args: &[Value], index: usize, op: &str) -> Result<String, HostError> {
    match args.get(index) {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        _ => Err(type_error(op, index, "a string")),
    }
}

fn op_context(ctx: &mut dyn CallContext) -> Result<&mut OpContext, HostError> {
    ctx.isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))
}

fn ws_arg(ctx: &mut dyn CallContext, args: &[Value], op: &str) -> Result<Arc<Ws>, HostError> {
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error(op, 0, "a resource id"));
    };
    let id = ResourceId::from_js_number(*id)?;
    Ok(op_context(ctx)?.resources.get::<WsRes>(id)?.0.clone())
}

/// Creates the connection resource; returns it with the half the socket task keeps.
fn create(ctx: &mut dyn CallContext, capability: Option<String>) -> Result<(f64, Link), HostError> {
    let op_ctx = op_context(ctx)?;
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel(EVENT_QUEUE);
    let ws = Arc::new(Ws {
        outgoing: out_tx,
        events: Mutex::new(event_rx),
        buffered: Arc::new(AtomicU64::new(0)),
        cancel: CancellationToken::new(),
    });
    let link = Link {
        outgoing: out_rx,
        events: event_tx,
        cancel: ws.cancel.clone(),
        quotas: op_ctx.quotas.clone(),
        buffered: ws.buffered.clone(),
    };
    let id = op_ctx
        .resources
        .insert(Box::new(WsRes(ws)), QuotaClass::Sockets, capability)?;
    Ok((id.to_js_number(), link))
}

fn fail(events: &mpsc::Sender<Event>, message: String) {
    let events = events.clone();
    tokio::spawn(async move {
        let _ = events.send(Event::Error(message)).await;
        let _ = events
            .send(Event::Close {
                code: 1006,
                reason: String::new(),
                clean: false,
            })
            .await;
    });
}

async fn connect(
    uri: Uri,
    protocols: Vec<String>,
    host: String,
    port: u16,
    ctx: ConnectCtx,
    link: &Link,
) -> Result<(WebSocketStream<Box<dyn Duplex>>, String), VelyrError> {
    let tcp = dial(&host, port, &ctx.permissions, &ctx.resolver).await?;
    let io: Box<dyn Duplex> = if uri.scheme_str() == Some("wss") {
        let tls = client_config(&[], vec![b"http/1.1".to_vec()])?;
        let name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|_| VelyrError::invalid_data("invalid TLS server name"))?;
        let stream = TlsConnector::from(Arc::new(tls))
            .connect(name, tcp)
            .await
            .map_err(|e| {
                VelyrError::new(velyr_runtime_errors::ErrorCode::Network, e.to_string())
            })?;
        Box::new(stream)
    } else {
        Box::new(tcp)
    };
    let mut request = uri
        .into_client_request()
        .map_err(|e| VelyrError::invalid_data(e.to_string()))?;
    if !protocols.is_empty() {
        let value = HeaderValue::from_str(&protocols.join(", "))
            .map_err(|_| VelyrError::invalid_data("invalid subprotocol"))?;
        request
            .headers_mut()
            .insert("sec-websocket-protocol", value);
    }
    let (stream, response) = client_async_with_config(request, io, Some(config(&link.quotas)))
        .await
        .map_err(|e| VelyrError::new(velyr_runtime_errors::ErrorCode::Network, e.to_string()))?;
    let protocol = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    Ok((stream, protocol))
}

struct ConnectCtx {
    permissions: Arc<velyr_runtime_permissions::CapabilitySet>,
    resolver: velyr_io_net::Resolver,
}

/// `op_ws_connect(url, protocolsJson) -> id`: starts the connection (needs `net-connect` for the
/// host); the outcome arrives as events. `url` is an absolute `ws:` / `wss:` URL.
fn op_ws_connect(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.ws_connect").entered();
    let url = string_arg(args, 0, "op_ws_connect")?;
    let protocols: Vec<String> = serde_json::from_str(&string_arg(args, 1, "op_ws_connect")?)
        .map_err(|_| type_error("op_ws_connect", 1, "a JSON list of protocols"))?;
    let uri: Uri = url
        .parse()
        .map_err(|_| type_error("op_ws_connect", 0, "a valid URL"))?;
    let secure = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err(type_error("op_ws_connect", 0, "a ws: or wss: URL")),
    };
    let host = uri
        .host()
        .ok_or_else(|| type_error("op_ws_connect", 0, "a URL with a host"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    authorize(ctx, "op_ws_connect", Request::NetConnect(&host, port))?;
    let connect_ctx = ConnectCtx {
        permissions: op_context(ctx)?.permissions.clone(),
        resolver: resolver_of(ctx)?,
    };
    let (id, link) = create(ctx, Some(format!("{host}:{port}")))?;
    tokio::spawn(async move {
        let attempt = tokio::select! {
            () = link.cancel.cancelled() => return,
            attempt = tokio::time::timeout(
                HANDSHAKE_TIMEOUT,
                connect(uri, protocols, host, port, connect_ctx, &link),
            ) => attempt,
        };
        match attempt {
            Ok(Ok((stream, protocol))) => {
                if link.events.send(Event::Open { protocol }).await.is_ok() {
                    drive(stream, link).await;
                }
            }
            Ok(Err(err)) => fail(&link.events, err.to_string()),
            Err(_) => fail(&link.events, "the opening handshake timed out".into()),
        }
    });
    Ok(Value::Number(id))
}

/// `op_ws_upgrade(requestId, protocol | null) -> id`: the connection of an `Velyr.serve` request
/// becomes a WebSocket once the handler returns the `101` response.
fn op_ws_upgrade(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.ws_upgrade").entered();
    let Some(Value::Number(request_id)) = args.first() else {
        return Err(type_error("op_ws_upgrade", 0, "a request id"));
    };
    let request_id = *request_id;
    let protocol = match args.get(1) {
        Some(Value::String(text)) => text.to_string_lossy(),
        _ => String::new(),
    };
    let (id, link) = create(ctx, None)?;
    let (io_tx, io_rx) = oneshot::channel::<Box<dyn Duplex>>();
    let attached = velyr_web_fetch::attach_upgrade(
        ctx,
        request_id,
        Box::new(move |io| {
            let _ = io_tx.send(io);
        }),
    );
    if let Err(err) = attached {
        // Roll back the resource created above; the caller sees the error.
        if let Ok(rid) = ResourceId::from_js_number(id) {
            let _ = op_context(ctx)?.resources.close(rid);
        }
        return Err(err);
    }
    let settings: WebSocketConfig = config(&link.quotas);
    tokio::spawn(async move {
        let io = tokio::select! {
            () = link.cancel.cancelled() => return,
            io = io_rx => io,
        };
        match io {
            Ok(io) => {
                let stream =
                    WebSocketStream::from_raw_socket(io, Role::Server, Some(settings)).await;
                if link.events.send(Event::Open { protocol }).await.is_ok() {
                    drive(stream, link).await;
                }
            }
            Err(_) => fail(&link.events, "the upgrade was not completed".into()),
        }
    });
    Ok(Value::Number(id))
}

/// `op_ws_next(id) -> Promise<string | ArrayBuffer | null>`: the next event as JSON
/// `{t: "open" | "text" | "error" | "close", ...}`, binary messages as bytes, `null` at the end.
fn op_ws_next(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let ws = ws_arg(ctx, args, "op_ws_next")?;
    op_async(ctx, move |token| async move {
        let mut events = ws.events.lock().await;
        let event = tokio::select! {
            () = token.cancelled() => return Ok(CompletionValue::Null),
            event = events.recv() => event,
        };
        Ok(match event {
            None => CompletionValue::Null,
            Some(Event::Binary(bytes, _guard)) => CompletionValue::Bytes(bytes),
            Some(Event::Text(text, _guard)) => {
                CompletionValue::String(serde_json::json!({"t": "text", "data": text}).to_string())
            }
            Some(Event::Open { protocol }) => CompletionValue::String(
                serde_json::json!({"t": "open", "protocol": protocol}).to_string(),
            ),
            Some(Event::Error(message)) => CompletionValue::String(
                serde_json::json!({"t": "error", "message": message}).to_string(),
            ),
            Some(Event::Close {
                code,
                reason,
                clean,
            }) => CompletionValue::String(
                serde_json::json!({"t": "close", "code": code, "reason": reason, "clean": clean})
                    .to_string(),
            ),
        })
    })
}

/// `op_ws_send(id, string | bytes)`: queues one message.
fn op_ws_send(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let ws = ws_arg(ctx, args, "op_ws_send")?;
    let message = match args.get(1) {
        Some(Value::String(text)) => Outgoing::Text(text.to_string_lossy()),
        Some(value @ Value::Object(_)) => Outgoing::Binary(
            ctx.read_bytes(value)
                .ok_or_else(|| type_error("op_ws_send", 1, "a string or bytes"))?,
        ),
        _ => return Err(type_error("op_ws_send", 1, "a string or bytes")),
    };
    let size = match &message {
        Outgoing::Text(text) => text.len(),
        Outgoing::Binary(bytes) => bytes.len(),
        Outgoing::Close(_) => 0,
    };
    ws.buffered.fetch_add(size as u64, Ordering::Relaxed);
    // A finished connection drops the message, like a send after close.
    if ws.outgoing.send(message).is_err() {
        ws.buffered.fetch_sub(size as u64, Ordering::Relaxed);
    }
    Ok(Value::Undefined)
}

/// `op_ws_buffered(id) -> number`: bytes queued and not written yet.
fn op_ws_buffered(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let ws = ws_arg(ctx, args, "op_ws_buffered")?;
    Ok(Value::Number(ws.buffered.load(Ordering::Relaxed) as f64))
}

/// `op_ws_close(id, code | null, reason)`: starts the closing handshake.
fn op_ws_close(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let ws = ws_arg(ctx, args, "op_ws_close")?;
    let close = match args.get(1) {
        Some(Value::Number(code)) if (1000.0..=4999.0).contains(code) => {
            Some((*code as u16, string_arg(args, 2, "op_ws_close")?))
        }
        _ => None,
    };
    let _ = ws.outgoing.send(Outgoing::Close(close));
    Ok(Value::Undefined)
}

/// `op_ws_release(id)`: drops the connection without a closing handshake.
fn op_ws_release(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error("op_ws_release", 0, "a resource id"));
    };
    let id = ResourceId::from_js_number(*id)?;
    let _ = op_context(ctx)?.resources.close(id);
    Ok(Value::Undefined)
}

/// `op_ws_accept_key(key) -> string`: the `Sec-WebSocket-Accept` value for a client key.
fn op_ws_accept_key(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let key = string_arg(args, 0, "op_ws_accept_key")?;
    Ok(ctx.create_string(derive_accept_key(key.as_bytes()).as_str().into()))
}

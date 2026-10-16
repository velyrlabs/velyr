//! Host ops behind the `node:http` / `node:https` client: `op_node_http_client_*` and
//! `op_node_http_creq_*`. An `Agent` owns one io-http `HttpClient` (its connection pool); a request
//! starts on the runtime as soon as `creq_begin` returns, its body is fed through `creq_write` /
//! `creq_end`, and the response head and body come back through `creq_response` / `creq_read`.
//! Connections go through io-net's two-phase connect authorization, so `net-connect` applies.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_io_http::{
    BodyReceiver, BodySender, ClientOptions, HttpClient, HttpRequestParts, HttpResponseParts,
    Limits, body_channel,
};
use velyr_io_net::{resolver_of, system_resolver};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::op_async;

use crate::http_ops::{op_context, string_arg, type_error};

/// Largest response body accepted.
const MAX_BODY_BYTES: u64 = 256 * 1024 * 1024;

macro_rules! op {
    ($name:ident, $arity:expr) => {
        HostFunctionDef {
            name: stringify!($name),
            arity: $arity,
            callback: $name,
        }
    };
}

pub(crate) fn http_client_host_functions() -> Vec<HostFunctionDef> {
    vec![
        op!(op_node_http_client_new, 1),
        op!(op_node_http_client_close, 1),
        op!(op_node_http_creq_begin, 5),
        op!(op_node_http_creq_write, 2),
        op!(op_node_http_creq_end, 1),
        op!(op_node_http_creq_response, 1),
        op!(op_node_http_creq_read, 1),
        op!(op_node_http_creq_close, 1),
    ]
}

/// Isolate slot: the agents' clients and the requests in flight, by id.
#[derive(Default)]
pub(crate) struct HttpState {
    clients: HashMap<u64, Arc<HttpClient>>,
    requests: HashMap<u64, Arc<ClientReq>>,
    next: u64,
}

type Response = Result<HttpResponseParts, VelyrError>;

struct ClientReq {
    cancel: CancellationToken,
    /// Feeds the request body until `end`.
    out: Mutex<Option<BodySender>>,
    response: Mutex<Option<oneshot::Receiver<Response>>>,
    body: Mutex<Option<BodyReceiver>>,
}

fn state(ctx: &mut dyn CallContext) -> Result<&mut HttpState, HostError> {
    ctx.isolate_slot::<HttpState>()
        .ok_or_else(|| HostError::TypeError("node:http needs install_compat_node".into()))
}

fn id_arg(args: &[Value], index: usize, op: &str) -> Result<u64, HostError> {
    match args.get(index) {
        Some(Value::Number(id)) if id.fract() == 0.0 && *id >= 0.0 => Ok(*id as u64),
        _ => Err(type_error(op, index, "an id")),
    }
}

fn request_arg(
    ctx: &mut dyn CallContext,
    args: &[Value],
    op: &str,
) -> Result<Arc<ClientReq>, HostError> {
    let id = id_arg(args, 0, op)?;
    state(ctx)?
        .requests
        .get(&id)
        .cloned()
        .ok_or_else(|| VelyrError::bad_resource().into())
}

/// `op_node_http_client_new(optionsJson) -> id`: options `{ca: [pem, ...]}` add trust roots on top
/// of the system ones. The client speaks HTTP/1.1 only, for plain and TLS connections.
fn op_node_http_client_new(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let op = "op_node_http_client_new";
    let options: serde_json::Value = serde_json::from_str(&string_arg(args, 0, op)?)
        .map_err(|_| type_error(op, 0, "a JSON options object"))?;
    let pems: Vec<String> = options["ca"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|pem| pem.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let extra: Vec<&str> = pems.iter().map(String::as_str).collect();
    let tls = match velyr_io_tls::client_config(&extra, vec![b"http/1.1".to_vec()]) {
        Ok(config) => Some(Arc::new(config)),
        // Without extra roots a missing platform verifier only breaks https; a bad `ca` is an error.
        Err(_) if extra.is_empty() => None,
        Err(err) => return Err(VelyrError::invalid_data(format!("invalid ca: {err}")).into()),
    };
    let context = op_context(ctx)?;
    let (permissions, quotas) = (context.permissions.clone(), context.quotas.clone());
    let resolver = resolver_of(ctx).unwrap_or_else(|_| system_resolver());
    let client = HttpClient::new(
        permissions,
        resolver,
        ClientOptions {
            limits: Limits {
                max_body_bytes: MAX_BODY_BYTES,
                quotas: Some(quotas),
                ..Limits::default()
            },
            tls,
            ..ClientOptions::default()
        },
    );
    let state = state(ctx)?;
    state.next += 1;
    let id = state.next;
    state.clients.insert(id, Arc::new(client));
    Ok(Value::Number(id as f64))
}

/// `op_node_http_client_close(id)`: drops the client and its idle connections.
fn op_node_http_client_close(
    ctx: &mut dyn CallContext,
    args: &[Value],
) -> Result<Value, HostError> {
    let id = id_arg(args, 0, "op_node_http_client_close")?;
    state(ctx)?.clients.remove(&id);
    Ok(Value::Undefined)
}

/// `op_node_http_creq_begin(clientId, method, url, headersJson, body) -> id`: starts the request;
/// `body` is `null`, bytes, or `true` for a body fed through `creq_write` / `creq_end`.
fn op_node_http_creq_begin(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let op = "op_node_http_creq_begin";
    let client_id = id_arg(args, 0, op)?;
    let method = string_arg(args, 1, op)?;
    let uri = string_arg(args, 2, op)?;
    let headers: Vec<(String, String)> = serde_json::from_str(&string_arg(args, 3, op)?)
        .map_err(|_| type_error(op, 3, "a JSON header list"))?;
    let (body, out) = match args.get(4) {
        Some(Value::Bool(true)) => {
            let (tx, rx) = body_channel();
            (rx, Some(tx))
        }
        Some(value @ Value::Object(_)) => {
            let bytes = ctx
                .read_bytes(value)
                .ok_or_else(|| type_error(op, 4, "bytes, true or null"))?;
            (BodyReceiver::once(bytes), None)
        }
        _ => (BodyReceiver::empty(), None),
    };
    let state = state(ctx)?;
    let client = state
        .clients
        .get(&client_id)
        .cloned()
        .ok_or_else(VelyrError::bad_resource)?;
    let (reply, response) = oneshot::channel();
    let request = Arc::new(ClientReq {
        cancel: CancellationToken::new(),
        out: Mutex::new(out),
        response: Mutex::new(Some(response)),
        body: Mutex::new(None),
    });
    state.next += 1;
    let id = state.next;
    state.requests.insert(id, request.clone());
    let parts = HttpRequestParts {
        remote_addr: None,
        method,
        uri,
        version: String::new(),
        headers,
        body,
    };
    tokio::spawn(async move {
        let sent = tokio::select! {
            () = request.cancel.cancelled() => Err(VelyrError::cancelled()),
            sent = client.request(parts) => sent,
        };
        let _ = reply.send(sent);
    });
    Ok(Value::Number(id as f64))
}

/// `op_node_http_creq_write(id, data) -> Promise<undefined>`: next piece of the request body;
/// waits while the connection is not ready for more.
fn op_node_http_creq_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let request = request_arg(ctx, args, "op_node_http_creq_write")?;
    let data = args
        .get(1)
        .and_then(|value| ctx.read_bytes(value))
        .ok_or_else(|| type_error("op_node_http_creq_write", 1, "an ArrayBuffer or view"))?;
    op_async(ctx, move |token| async move {
        let guard = request.out.lock().await;
        let tx = guard.as_ref().ok_or_else(VelyrError::bad_resource)?;
        let sent = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            () = request.cancel.cancelled() => return Err(VelyrError::cancelled()),
            sent = tx.send(Ok(data.into())) => sent,
        };
        if !sent {
            return Err(VelyrError::new(
                velyr_runtime_errors::ErrorCode::Network,
                "the server closed the connection",
            ));
        }
        Ok(CompletionValue::Undefined)
    })
}

/// `op_node_http_creq_end(id)`: the request body is complete.
fn op_node_http_creq_end(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let request = request_arg(ctx, args, "op_node_http_creq_end")?;
    if let Ok(mut out) = request.out.try_lock() {
        out.take();
    } else {
        // A write is still waiting for room: end once it is done.
        tokio::spawn(async move {
            request.out.lock().await.take();
        });
    }
    Ok(Value::Undefined)
}

/// `op_node_http_creq_response(id) -> Promise<string>`: waits for the response head; JSON
/// `{status, headers}`.
fn op_node_http_creq_response(
    ctx: &mut dyn CallContext,
    args: &[Value],
) -> Result<Value, HostError> {
    let request = request_arg(ctx, args, "op_node_http_creq_response")?;
    op_async(ctx, move |token| async move {
        let rx = request
            .response
            .lock()
            .await
            .take()
            .ok_or_else(VelyrError::bad_resource)?;
        let head = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            head = rx => head.map_err(|_| VelyrError::cancelled())??,
        };
        let json = serde_json::json!({"status": head.status, "headers": head.headers});
        *request.body.lock().await = Some(head.body);
        Ok(CompletionValue::String(json.to_string()))
    })
}

/// `op_node_http_creq_read(id) -> Promise<ArrayBuffer | null>`: next piece of the response body.
fn op_node_http_creq_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let request = request_arg(ctx, args, "op_node_http_creq_read")?;
    op_async(ctx, move |token| async move {
        let mut guard = request.body.lock().await;
        let body = guard.as_mut().ok_or_else(VelyrError::bad_resource)?;
        let chunk = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            () = request.cancel.cancelled() => return Err(VelyrError::cancelled()),
            chunk = body.recv() => chunk,
        };
        match chunk {
            None => Ok(CompletionValue::Null),
            Some(chunk) => Ok(CompletionValue::Bytes(chunk?.to_vec())),
        }
    })
}

/// `op_node_http_creq_close(id)`: cancels the request if it is still running and forgets it.
fn op_node_http_creq_close(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = id_arg(args, 0, "op_node_http_creq_close")?;
    if let Some(request) = state(ctx)?.requests.remove(&id) {
        request.cancel.cancel();
    }
    Ok(Value::Undefined)
}

//! The `op_fetch_*` ops. One request is one resource: `begin` authorizes the hop and reserves it,
//! `send` returns the response head, `read` streams the (decoded) body, `write`/`end` feed a
//! streamed request body and `close` cancels whatever is pending. Redirects are the caller's loop,
//! so every hop is authorized again.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use http::{StatusCode, Uri};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_io_http::{BodyReceiver, BodySender, HttpClient, HttpRequestParts, body_channel};
use velyr_io_net::resolver_of;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{CapabilityKind, OpContext, authorize, op_async};
use velyr_runtime_permissions::Request;
use velyr_runtime_resources::{QuotaClass, QuotaGuard, Resource, ResourceId};

use crate::client::{FetchState, new_client};
use crate::decode::Decoder;

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

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 9] = [
    op!(op_fetch_begin, 1, NetConnect),
    op!(op_fetch_send, 6, None),
    op!(op_fetch_detach, 1, None),
    op!(op_fetch_read, 1, None),
    op!(op_fetch_write, 2, None),
    op!(op_fetch_end, 1, None),
    op!(op_fetch_close, 1, None),
    op!(op_fetch_release, 2, None),
    op!(op_fetch_abort_upload, 1, None),
];

struct ResponseBody {
    rx: BodyReceiver,
    decoder: Option<Decoder>,
    done: bool,
}

/// State of one request, shared with the tasks the ops spawn.
///
/// Two lifetimes meet here. The response ends when its body is finished or released; the upload
/// ends when `op_fetch_end`/`op_fetch_abort_upload` run. The request is cancelled once both are
/// over, or at once when the caller releases the body with `hard` (abort, close).
pub(crate) struct Fetch {
    /// Cancelled when the resource closes: every pending op of this request ends.
    cancel: CancellationToken,
    upload: Mutex<Option<BodySender>>,
    body: Mutex<Option<ResponseBody>>,
    /// Set when the body moved to the isolate's `bodies`: closing the table entry then must not
    /// cancel the request.
    detached: AtomicBool,
    /// A streamed request body was started and has not ended or been aborted yet.
    upload_active: AtomicBool,
    /// The response body was dropped; only the upload may still be running.
    response_gone: AtomicBool,
}

impl Fetch {
    /// The upload is over (completed or aborted): cancels the request if nothing else needs it.
    fn upload_over(&self) {
        self.upload_active.store(false, Ordering::SeqCst);
        if self.response_gone.load(Ordering::SeqCst) {
            self.cancel.cancel();
        }
    }
}

/// A response body JS holds after the head arrived. Dropping it (the body finished, was released,
/// or the isolate ended) gives its `sockets` unit back and cancels the request, unless the
/// request body is still being uploaded: that goes on until it ends.
pub(crate) struct DetachedBody {
    fetch: Arc<Fetch>,
    _guard: QuotaGuard,
}

impl Drop for DetachedBody {
    fn drop(&mut self) {
        self.fetch.response_gone.store(true, Ordering::SeqCst);
        if !self.fetch.upload_active.load(Ordering::SeqCst) {
            self.fetch.cancel.cancel();
        }
    }
}

struct FetchRes(Arc<Fetch>);

impl Resource for FetchRes {
    fn kind(&self) -> &'static str {
        "fetch"
    }

    fn close(self: Box<Self>) {
        if !self.0.detached.load(Ordering::Acquire) {
            self.0.cancel.cancel();
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub(crate) fn type_error(op: &str, index: usize, what: &str) -> HostError {
    HostError::TypeError(format!("{op} expects argument {index} to be {what}"))
}

pub(crate) fn string_arg(args: &[Value], index: usize, op: &str) -> Result<String, HostError> {
    match args.get(index) {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        _ => Err(type_error(op, index, "a string")),
    }
}

pub(crate) fn op_context(ctx: &mut dyn CallContext) -> Result<&mut OpContext, HostError> {
    ctx.isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))
}

fn fetch_arg(ctx: &mut dyn CallContext, args: &[Value], op: &str) -> Result<Arc<Fetch>, HostError> {
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error(op, 0, "a request id"));
    };
    let id = ResourceId::from_js_number(*id)?;
    Ok(op_context(ctx)?.resources.get::<FetchRes>(id)?.0.clone())
}

/// The request behind the id of a streamed upload: still in the resource table, or, once the
/// response body was detached, in the isolate's `uploads`.
fn upload_arg(
    ctx: &mut dyn CallContext,
    args: &[Value],
    op: &str,
) -> Result<Arc<Fetch>, HostError> {
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error(op, 0, "a request id"));
    };
    let id = ResourceId::from_js_number(*id)?;
    let listed = op_context(ctx)?
        .resources
        .get::<FetchRes>(id)
        .map(|res| res.0.clone());
    if let Ok(fetch) = listed {
        return Ok(fetch);
    }
    let state = ctx.isolate_slot::<FetchState>();
    match state.and_then(|state| state.uploads.get(&id)) {
        Some(fetch) => Ok(fetch.clone()),
        None => Err(VelyrError::bad_resource().into()),
    }
}

fn body_arg(ctx: &mut dyn CallContext, args: &[Value], op: &str) -> Result<Arc<Fetch>, HostError> {
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error(op, 0, "a body id"));
    };
    let state = ctx.isolate_slot::<FetchState>();
    match state.and_then(|state| state.bodies.get(&(*id as u64))) {
        Some(body) => Ok(body.fetch.clone()),
        None => Err(VelyrError::bad_resource().into()),
    }
}

/// `host` and `port` of an `http(s)` URL.
fn target(url: &str) -> Result<(String, u16), VelyrError> {
    let uri: Uri = url
        .parse()
        .map_err(|_| VelyrError::invalid_data("invalid request URL"))?;
    let port = match (uri.port_u16(), uri.scheme_str()) {
        (Some(port), _) => port,
        (None, Some("https")) => 443,
        (None, Some("http")) => 80,
        _ => {
            return Err(VelyrError::invalid_data(
                "only http and https URLs can be fetched",
            ));
        }
    };
    let host = uri
        .host()
        .ok_or_else(|| VelyrError::invalid_data("request URL has no host"))?;
    Ok((host.to_owned(), port))
}

/// `op_fetch_begin(url) -> id`: authorizes `net-connect` for this hop's host, then reserves a
/// request. The caller must `close` it. The open request holds a connection, so it counts as a
/// `sockets` unit.
fn op_fetch_begin(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.fetch_begin").entered();
    let (host, port) = target(&string_arg(args, 0, "op_fetch_begin")?)?;
    authorize(ctx, "op_fetch_begin", Request::NetConnect(&host, port))?;
    let fetch = Arc::new(Fetch {
        cancel: CancellationToken::new(),
        upload: Mutex::new(None),
        body: Mutex::new(None),
        detached: AtomicBool::new(false),
        upload_active: AtomicBool::new(false),
        response_gone: AtomicBool::new(false),
    });
    let id = op_context(ctx)?.resources.insert(
        Box::new(FetchRes(fetch)),
        QuotaClass::Sockets,
        Some(format!("{host}:{port}")),
    )?;
    Ok(Value::Number(id.to_js_number()))
}

fn client_of(ctx: &mut dyn CallContext) -> Result<Arc<HttpClient>, HostError> {
    if let Some(client) = ctx
        .isolate_slot::<FetchState>()
        .and_then(|s| s.client.clone())
    {
        return Ok(client);
    }
    let (permissions, quotas) = {
        let op_ctx = op_context(ctx)?;
        (op_ctx.permissions.clone(), op_ctx.quotas.clone())
    };
    let client = new_client(permissions, resolver_of(ctx)?, quotas)?;
    if let Some(state) = ctx.isolate_slot::<FetchState>() {
        state.client = Some(client.clone());
    }
    Ok(client)
}

fn headers_arg(args: &[Value]) -> Result<Vec<(String, String)>, HostError> {
    let json = string_arg(args, 3, "op_fetch_send")?;
    serde_json::from_str(&json).map_err(|_| type_error("op_fetch_send", 3, "a JSON header list"))
}

/// `op_fetch_send(id, method, url, headersJson, body, parentSpanId) -> Promise<string>`: `body` is
/// `null`, bytes, or `true` for a body fed through `op_fetch_write`; the span id is a number or `null`. Resolves JSON `{status, statusText, headers}`.
fn op_fetch_send(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.fetch_send").entered();
    let fetch = fetch_arg(ctx, args, "op_fetch_send")?;
    let method = string_arg(args, 1, "op_fetch_send")?;
    #[cfg_attr(not(feature = "telemetry"), allow(unused_mut))]
    let mut headers = headers_arg(args)?;
    let uri = string_arg(args, 2, "op_fetch_send")?;
    #[cfg(feature = "telemetry")]
    let span = {
        let parent = match args.get(5) {
            Some(Value::Number(id)) => crate::telemetry::lookup(*id as u64),
            _ => None,
        };
        velyr_runtime_telemetry::client_span(&method, &uri, &mut headers, parent.as_ref())
    };
    let (body, upload) = match args.get(4) {
        Some(Value::Bool(true)) => {
            let (tx, rx) = body_channel();
            (rx, Some(tx))
        }
        Some(value @ Value::Object(_)) => {
            let bytes = ctx
                .read_bytes(value)
                .ok_or_else(|| type_error("op_fetch_send", 4, "bytes, true or null"))?;
            (BodyReceiver::once(bytes), None)
        }
        _ => (BodyReceiver::empty(), None),
    };
    // Set before the task runs: the caller starts writing as soon as this returns.
    if let Some(tx) = upload {
        fetch.upload_active.store(true, Ordering::SeqCst);
        *fetch
            .upload
            .try_lock()
            .map_err(|_| VelyrError::internal("request body slot is busy"))? = Some(tx);
    }
    let decode = method != "HEAD";
    let parts = HttpRequestParts {
        remote_addr: None,
        method,
        uri,
        version: String::new(),
        headers,
        body,
    };
    let client = client_of(ctx)?;
    op_async(ctx, move |token| async move {
        let response = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            () = fetch.cancel.cancelled() => return Err(VelyrError::cancelled()),
            response = client.request(parts) => response,
        };
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                #[cfg(feature = "telemetry")]
                if let Some(span) = span {
                    span.fail(&err.to_string());
                }
                return Err(err);
            }
        };
        #[cfg(feature = "telemetry")]
        if let Some(span) = span {
            span.finish(response.status, 400);
        }
        let decoder = decoder_for(decode, response.status, &response.headers);
        let head = serde_json::json!({
            "status": response.status,
            "statusText": StatusCode::from_u16(response.status).ok()
                .and_then(|status| status.canonical_reason()).unwrap_or(""),
            "headers": response.headers,
        });
        *fetch.body.lock().await = Some(ResponseBody {
            rx: response.body,
            decoder,
            done: false,
        });
        Ok(CompletionValue::String(head.to_string()))
    })
}

/// The decoder for a response that has a body and says it is encoded.
fn decoder_for(decode: bool, status: u16, headers: &[(String, String)]) -> Option<Decoder> {
    if !decode || matches!(status, 100..=199 | 204 | 205 | 304) {
        return None;
    }
    let codings: Vec<&str> = headers
        .iter()
        .filter(|(name, _)| name == "content-encoding")
        .map(|(_, value)| value.as_str())
        .collect();
    Decoder::for_encoding(&codings.join(","))
}

/// `op_fetch_detach(id) -> bodyId`: once the response head is in, the body leaves the resource
/// table for the isolate's own map and the request id is closed.
///
/// Why: a script may drop a `Response` without reading its body, and there is no end-of-loop hook
/// or guaranteed GC to close it by then. Held in the table it would be reported as a leak at
/// shutdown; held here it is released when the body finishes, when `op_fetch_release` runs
/// (JS calls it from a `FinalizationRegistry` when the body stream is collected, if the engine has
/// one), or at the latest when the isolate ends and drops the map. The `sockets` unit stays
/// reserved by the guard until then.
fn op_fetch_detach(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error("op_fetch_detach", 0, "a request id"));
    };
    let id = ResourceId::from_js_number(*id)?;
    let (fetch, quotas) = {
        let op_ctx = op_context(ctx)?;
        (
            op_ctx.resources.get::<FetchRes>(id)?.0.clone(),
            op_ctx.quotas.clone(),
        )
    };
    fetch.detached.store(true, Ordering::Release);
    op_context(ctx)?.resources.close(id)?;
    let guard = match quotas.reserve(QuotaClass::Sockets, 1) {
        Ok(guard) => guard,
        Err(err) => {
            fetch.cancel.cancel();
            return Err(err.into());
        }
    };
    let uploading = fetch
        .upload_active
        .load(Ordering::SeqCst)
        .then(|| fetch.clone());
    let body = DetachedBody {
        fetch,
        _guard: guard,
    };
    let state = ctx
        .isolate_slot::<FetchState>()
        .ok_or_else(|| VelyrError::internal("fetch is not installed"))?;
    if let Some(fetch) = uploading {
        state.uploads.insert(id, fetch);
    }
    state.next_body += 1;
    let body_id = state.next_body;
    state.bodies.insert(body_id, body);
    Ok(Value::Number(body_id as f64))
}

/// `op_fetch_release(bodyId, hard?)`: forgets a detached body; unknown ids are an error. The
/// request is cancelled unless an upload is still running, or at once when `hard` is `true`.
fn op_fetch_release(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let hard = matches!(args.get(1), Some(Value::Bool(true)));
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error("op_fetch_release", 0, "a body id"));
    };
    let state = ctx.isolate_slot::<FetchState>();
    match state.and_then(|state| state.bodies.remove(&(*id as u64))) {
        Some(body) => {
            if hard {
                body.fetch.cancel.cancel();
            }
            Ok(Value::Undefined)
        }
        None => Err(VelyrError::bad_resource().into()),
    }
}

/// `op_fetch_read(id) -> Promise<ArrayBuffer | null>`: next piece of the decoded body; `null` at
/// the end. A cut-short compressed stream or a closed request is an error.
fn op_fetch_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.fetch_read").entered();
    let fetch = body_arg(ctx, args, "op_fetch_read")?;
    op_async(ctx, move |token| async move {
        let mut guard = fetch.body.lock().await;
        let body = guard.as_mut().ok_or_else(VelyrError::bad_resource)?;
        loop {
            if body.done {
                return Ok(CompletionValue::Null);
            }
            let chunk = tokio::select! {
                () = token.cancelled() => return Err(VelyrError::cancelled()),
                () = fetch.cancel.cancelled() => return Err(VelyrError::cancelled()),
                chunk = body.rx.recv() => chunk,
            };
            let out = match chunk {
                None => {
                    body.done = true;
                    body.decoder
                        .as_mut()
                        .map_or_else(|| Ok(Vec::new()), Decoder::finish)?
                }
                Some(chunk) => {
                    let bytes = chunk?;
                    match body.decoder.as_mut() {
                        Some(decoder) => decoder.push(&bytes)?,
                        None => bytes.to_vec(),
                    }
                }
            };
            if !out.is_empty() {
                return Ok(CompletionValue::Bytes(out));
            }
        }
    })
}

/// `op_fetch_write(id, data) -> Promise<undefined>`: next piece of a streamed request body;
/// waits while the connection is not ready for more.
fn op_fetch_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let fetch = upload_arg(ctx, args, "op_fetch_write")?;
    let data = args
        .get(1)
        .and_then(|value| ctx.read_bytes(value))
        .ok_or_else(|| type_error("op_fetch_write", 1, "an ArrayBuffer or ArrayBufferView"))?;
    op_async(ctx, move |token| async move {
        let guard = fetch.upload.lock().await;
        let tx = guard.as_ref().ok_or_else(VelyrError::bad_resource)?;
        let sent = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            () = fetch.cancel.cancelled() => return Err(VelyrError::cancelled()),
            sent = tx.send(Ok(data.into())) => sent,
        };
        if !sent {
            return Err(VelyrError::new(
                velyr_runtime_errors::ErrorCode::Network,
                "the server stopped reading the request body",
            ));
        }
        Ok(CompletionValue::Undefined)
    })
}

/// `op_fetch_end(id) -> Promise<undefined>`: the streamed request body is complete.
fn op_fetch_end(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let fetch = upload_arg(ctx, args, "op_fetch_end")?;
    forget_upload(ctx, args);
    op_async(ctx, move |_token| async move {
        fetch.upload.lock().await.take();
        fetch.upload_over();
        Ok(CompletionValue::Undefined)
    })
}

/// `op_fetch_abort_upload(id) -> Promise<undefined>`: the source of the streamed request body
/// failed. The server is told the body is broken (not that it ended), and the request goes on
/// only as long as its response is still wanted.
fn op_fetch_abort_upload(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let fetch = upload_arg(ctx, args, "op_fetch_abort_upload")?;
    forget_upload(ctx, args);
    op_async(ctx, move |_token| async move {
        if let Some(tx) = fetch.upload.lock().await.take() {
            let _ = tx.send(Err(VelyrError::cancelled())).await;
        }
        fetch.upload_over();
        Ok(CompletionValue::Undefined)
    })
}

fn forget_upload(ctx: &mut dyn CallContext, args: &[Value]) {
    if let Some(Value::Number(id)) = args.first()
        && let Ok(id) = ResourceId::from_js_number(*id)
        && let Some(state) = ctx.isolate_slot::<FetchState>()
    {
        state.uploads.remove(&id);
    }
}

/// `op_fetch_close(id)`: cancels every pending op of the request and releases it.
fn op_fetch_close(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error("op_fetch_close", 0, "a request id"));
    };
    op_context(ctx)?
        .resources
        .close(ResourceId::from_js_number(*id)?)?;
    Ok(Value::Undefined)
}

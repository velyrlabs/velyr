use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_io_net::{
    ConnInner, begin_connect, conn_by_id, conn_info, dial, insert_upgrading, resolver_of, take_conn,
};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{CapabilityKind, OpContext, op_async};
use velyr_runtime_resources::ResourceId;

use crate::tls::{
    ClientOptions, TlsState, alpn_list, handshake_error, handshake_timeout, server_config,
};

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

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 7] = [
    op!(op_tls_connect_begin, 2, NetConnect),
    op!(op_tls_connect_finish, 2, None),
    op!(op_tls_start_begin, 1, None),
    op!(op_tls_start_finish, 2, None),
    op!(op_tls_server_config, 3, None),
    op!(op_tls_config_release, 1, None),
    op!(op_tls_accept_finish, 3, None),
];

fn string_arg(args: &[Value], index: usize, op: &str) -> Result<String, HostError> {
    match args.get(index) {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        _ => Err(HostError::TypeError(format!(
            "{op} expects argument {index} to be a string"
        ))),
    }
}

fn number_arg(args: &[Value], index: usize, op: &str) -> Result<f64, HostError> {
    match args.get(index) {
        Some(Value::Number(value)) => Ok(*value),
        _ => Err(HostError::TypeError(format!(
            "{op} expects argument {index} to be a number"
        ))),
    }
}

fn id_arg(args: &[Value], index: usize, op: &str) -> Result<ResourceId, HostError> {
    Ok(ResourceId::from_js_number(number_arg(args, index, op)?)?)
}

fn tls_state(ctx: &mut dyn CallContext) -> Result<&mut TlsState, HostError> {
    ctx.isolate_slot::<TlsState>()
        .ok_or_else(|| HostError::TypeError("tls ops need an installed TlsState".into()))
}

/// `{local, remote, alpnProtocol}` of a freshly negotiated connection over `base`.
fn handshake_info(base: &ConnInner, alpn: Option<&[u8]>) -> Result<CompletionValue, VelyrError> {
    let mut info = conn_info(base.tcp()?)?;
    info["alpnProtocol"] = alpn
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .into();
    Ok(CompletionValue::String(info.to_string()))
}

async fn connect_over(
    inner: Arc<ConnInner>,
    base: Arc<ConnInner>,
    options: ClientOptions,
) -> Result<CompletionValue, VelyrError> {
    let name = ServerName::try_from(options.hostname.clone())
        .map_err(|_| VelyrError::invalid_data("TLS hostname is not a valid server name"))?;
    let connector = TlsConnector::from(Arc::new(options.config()?));
    let tls = within(
        options.handshake_timeout,
        connector.connect(name, base.tcp_io()),
    )
    .await?;
    let info = handshake_info(&base, tls.get_ref().1.alpn_protocol())?;
    inner.fill_stream(tls, base)?;
    Ok(info)
}

/// Bounds a handshake; a peer that stalls fails with `VELYR_TIMEOUT`.
async fn within<T>(
    limit: Duration,
    handshake: impl Future<Output = std::io::Result<T>>,
) -> Result<T, VelyrError> {
    tokio::time::timeout(limit, handshake)
        .await
        .map_err(|_| VelyrError::timeout("TLS handshake timed out"))?
        .map_err(handshake_error)
}

/// Runs `work` unless the op or the connection is cancelled first.
async fn until_cancelled(
    token: tokio_util::sync::CancellationToken,
    inner: &ConnInner,
    work: impl Future<Output = Result<CompletionValue, VelyrError>>,
) -> Result<CompletionValue, VelyrError> {
    tokio::select! {
        () = token.cancelled() => Err(VelyrError::cancelled()),
        () = inner.cancelled() => Err(VelyrError::bad_resource()),
        result = work => result,
    }
}

/// `op_tls_connect_begin(hostname, port) -> id`: phase 1 and the socket placeholder, as io-net does.
fn op_tls_connect_begin(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.tls_connect_begin").entered();
    let host = string_arg(args, 0, "op_tls_connect_begin")?;
    let port = number_arg(args, 1, "op_tls_connect_begin")?;
    if port.fract() != 0.0 || !(0.0..=65535.0).contains(&port) {
        return Err(HostError::TypeError(
            "op_tls_connect_begin expects argument 1 to be a port number".into(),
        ));
    }
    let id = begin_connect(ctx, "op_tls_connect_begin", host, port as u16)?;
    Ok(Value::Number(id.to_js_number()))
}

/// `op_tls_connect_finish(id, options) -> Promise<string>`: resolve, phase 2, dial, handshake;
/// JSON `{local, remote, alpnProtocol}`.
fn op_tls_connect_finish(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.tls_connect_finish").entered();
    let inner = conn_by_id(ctx, id_arg(args, 0, "op_tls_connect_finish")?)?;
    let options = ClientOptions::parse(&string_arg(args, 1, "op_tls_connect_finish")?)?;
    let permissions = ctx
        .isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))?
        .permissions
        .clone();
    let resolver = resolver_of(ctx)?;
    op_async(ctx, |token| async move {
        let (host, port) = inner.target.clone();
        let work = async {
            let stream = dial(&host, port, &permissions, &resolver).await?;
            connect_over(
                inner.clone(),
                Arc::new(ConnInner::accepted(stream)),
                options,
            )
            .await
        };
        until_cancelled(token, &inner, work).await
    })
}

/// `op_tls_start_begin(connId) -> id`: moves the plain socket out of the table into a placeholder
/// that `start_finish` / `accept_finish` turn into the TLS connection.
fn op_tls_start_begin(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let base = take_conn(ctx, id_arg(args, 0, "op_tls_start_begin")?)?;
    let id = insert_upgrading(ctx, base)?;
    Ok(Value::Number(id.to_js_number()))
}

/// `op_tls_start_finish(id, options) -> Promise<string>`: client handshake over the upgraded socket.
fn op_tls_start_finish(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.tls_start_finish").entered();
    let inner = conn_by_id(ctx, id_arg(args, 0, "op_tls_start_finish")?)?;
    let options = ClientOptions::parse(&string_arg(args, 1, "op_tls_start_finish")?)?;
    let base = inner.take_upgrade_base()?;
    op_async(ctx, |token| async move {
        until_cancelled(token, &inner, connect_over(inner.clone(), base, options)).await
    })
}

/// `op_tls_server_config(certPem, keyPem, alpnJson) -> number`: validates and stores a server config.
fn op_tls_server_config(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let cert = string_arg(args, 0, "op_tls_server_config")?;
    let key = string_arg(args, 1, "op_tls_server_config")?;
    let alpn = serde_json::json!({ "alpnProtocols": serde_json::from_str::<serde_json::Value>(&string_arg(args, 2, "op_tls_server_config")?)
        .map_err(|_| VelyrError::invalid_data("invalid TLS options"))? });
    let config = server_config(&cert, &key, alpn_list(&alpn)?)?;
    Ok(Value::Number(tls_state(ctx)?.add_server(config) as f64))
}

fn op_tls_config_release(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = number_arg(args, 0, "op_tls_config_release")?;
    tls_state(ctx)?.release_server(id as u64);
    Ok(Value::Undefined)
}

/// `op_tls_accept_finish(id, configId, timeoutMs | null) -> Promise<string>`: server handshake over the upgraded socket.
fn op_tls_accept_finish(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.tls_accept_finish").entered();
    let inner = conn_by_id(ctx, id_arg(args, 0, "op_tls_accept_finish")?)?;
    let config = tls_state(ctx)?.server(number_arg(args, 1, "op_tls_accept_finish")? as u64)?;
    let limit = handshake_timeout(
        match args.get(2) {
            Some(Value::Number(ms)) => Some(serde_json::json!(ms)),
            _ => None,
        }
        .as_ref(),
    )?;
    let base = inner.take_upgrade_base()?;
    op_async(ctx, |token| async move {
        let work = async {
            let tls = within(limit, TlsAcceptor::from(config).accept(base.tcp_io())).await?;
            let info = handshake_info(&base, tls.get_ref().1.alpn_protocol())?;
            inner.fill_stream(tls, base)?;
            Ok(info)
        };
        until_cancelled(token, &inner, work).await
    })
}

//! Host ops for the connection an HTTP `101` response hands over (`op_node_http_upgrade` and
//! `op_node_http_up_*`), split from `http_ops.rs`. `upgrade` registers the hand-over before the
//! response is sent; `up_ready` waits for it; `up_read` / `up_write` / `up_shutdown` then use the
//! connection as a plain byte stream.

use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{Mutex, oneshot};
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_io_net::Duplex;
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::op_async;

use crate::http_ops::{network, req_arg, type_error};

/// Largest piece read from an upgraded connection at once.
const MAX_UP_READ: usize = 64 * 1024;

macro_rules! op {
    ($name:ident, $arity:expr) => {
        HostFunctionDef {
            name: stringify!($name),
            arity: $arity,
            callback: $name,
        }
    };
}

pub(crate) fn http_upgrade_host_functions() -> Vec<HostFunctionDef> {
    vec![
        op!(op_node_http_upgrade, 1),
        op!(op_node_http_up_ready, 1),
        op!(op_node_http_up_read, 1),
        op!(op_node_http_up_write, 2),
        op!(op_node_http_up_shutdown, 1),
    ]
}

/// The connection a `101` response handed over, once `up_ready` received it.
#[derive(Default)]
pub(crate) struct Upgraded {
    read: Mutex<Option<ReadHalf<Box<dyn Duplex>>>>,
    write: Mutex<Option<WriteHalf<Box<dyn Duplex>>>>,
}

/// `op_node_http_upgrade(id)`: the connection of this request becomes available through `up_*`
/// once it is answered with a `101` response.
fn op_node_http_upgrade(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_node_http_upgrade")?;
    let (tx, rx) = oneshot::channel::<Box<dyn Duplex>>();
    *req.upgrade.lock().expect("upgrade slot") = Some(Box::new(move |io| {
        let _ = tx.send(io);
    }));
    *req.upgrade_rx
        .try_lock()
        .map_err(|_| VelyrError::internal("the upgrade slot is busy"))? = Some(rx);
    Ok(Value::Undefined)
}

/// `op_node_http_up_ready(id) -> Promise<undefined>`: waits until the `101` response went out and
/// the connection was handed over.
fn op_node_http_up_ready(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_node_http_up_ready")?;
    op_async(ctx, move |token| async move {
        let rx = req
            .upgrade_rx
            .lock()
            .await
            .take()
            .ok_or_else(VelyrError::bad_resource)?;
        let io = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            () = req.cancel.cancelled() => return Err(VelyrError::cancelled()),
            io = rx => io.map_err(|_| VelyrError::new(ErrorCode::Network, "the upgrade was not completed"))?,
        };
        let (read, write) = tokio::io::split(io);
        *req.upgraded.read.lock().await = Some(read);
        *req.upgraded.write.lock().await = Some(write);
        Ok(CompletionValue::Undefined)
    })
}

/// `op_node_http_up_read(id) -> Promise<ArrayBuffer | null>`: next bytes from the upgraded
/// connection; `null` at EOF.
fn op_node_http_up_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_node_http_up_read")?;
    op_async(ctx, move |token| async move {
        let mut guard = req.upgraded.read.lock().await;
        let read = guard.as_mut().ok_or_else(VelyrError::bad_resource)?;
        let mut buf = vec![0u8; MAX_UP_READ];
        let n = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            () = req.cancel.cancelled() => return Err(VelyrError::cancelled()),
            n = read.read(&mut buf) => n.map_err(|e| network(&e))?,
        };
        if n == 0 {
            return Ok(CompletionValue::Null);
        }
        buf.truncate(n);
        Ok(CompletionValue::Bytes(buf))
    })
}

/// `op_node_http_up_write(id, data) -> Promise<undefined>`: writes and flushes `data`.
fn op_node_http_up_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_node_http_up_write")?;
    let data = args
        .get(1)
        .and_then(|value| ctx.read_bytes(value))
        .ok_or_else(|| type_error("op_node_http_up_write", 1, "an ArrayBuffer or view"))?;
    op_async(ctx, move |token| async move {
        let mut guard = req.upgraded.write.lock().await;
        let write = guard.as_mut().ok_or_else(VelyrError::bad_resource)?;
        tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            () = req.cancel.cancelled() => return Err(VelyrError::cancelled()),
            done = async { write.write_all(&data).await?; write.flush().await } => done.map_err(|e| network(&e))?,
        }
        Ok(CompletionValue::Undefined)
    })
}

/// `op_node_http_up_shutdown(id) -> Promise<undefined>`: half-closes the write side.
fn op_node_http_up_shutdown(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let req = req_arg(ctx, args, "op_node_http_up_shutdown")?;
    op_async(ctx, move |_token| async move {
        let mut guard = req.upgraded.write.lock().await;
        if let Some(write) = guard.as_mut() {
            let _ = write.shutdown().await;
        }
        Ok(CompletionValue::Undefined)
    })
}

//! Worker side of the message channel: an isolate slot plus the `op_worker_*` host functions.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::op_async;

use crate::channel::{Endpoint, Fill, Inbox, Packet, Tx};

/// Bounds the per-entry property reads a hostile `length` could request.
const MAX_TRANSFER_LIST: f64 = 65_536.0;

pub(crate) struct WorkerPort {
    pub(crate) inbox: Inbox,
    pub(crate) outbox: Tx,
    pub(crate) shutdown: CancellationToken,
    /// Exit code the worker asked for (`self.close()`, `process.exit`); read when the thread ends.
    pub(crate) exit: ExitCode,
}

/// Shared between the worker's isolate thread and the thread wrapper that reports its end.
pub(crate) type ExitCode = Arc<Mutex<Option<i32>>>;

/// Records `code` unless the worker already asked for another.
pub(crate) fn request_exit(exit: &ExitCode, code: i32) {
    exit.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_or_insert(code);
}

impl WorkerPort {
    /// The worker's side of the parent channel, as an endpoint the port table can own.
    pub(crate) fn endpoint(&self) -> Endpoint {
        Endpoint::direct(
            self.outbox.clone(),
            self.inbox.clone(),
            Some(self.shutdown.clone()),
        )
    }
}

/// Messaging ops installed on every worker. They stay on the global object until the
/// host's bootstrap captures and deletes them (ADR-010):
///
/// - `op_worker_post_message(value, transfer?)` structured-clones `value` to the parent;
/// - `op_worker_recv_message()` resolves `true` once a message is ready, `false` when the
///   parent closed the channel;
/// - `op_worker_take_message()` deserializes the ready message (`undefined` if none).
pub fn worker_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_worker_post_message",
            arity: 2,
            callback: op_worker_post_message,
        },
        HostFunctionDef {
            name: "op_worker_recv_message",
            arity: 0,
            callback: op_worker_recv_message,
        },
        HostFunctionDef {
            name: "op_worker_take_message",
            arity: 0,
            callback: op_worker_take_message,
        },
    ]
}

pub(crate) fn port(ctx: &mut dyn CallContext) -> Result<&mut WorkerPort, HostError> {
    ctx.isolate_slot::<WorkerPort>()
        .ok_or_else(|| HostError::TypeError("worker ops need a worker isolate".into()))
}

fn op_worker_post_message(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    if port(ctx)?.shutdown.is_cancelled() {
        return Err(VelyrError::cancelled().into());
    }
    let value = args.first().cloned().unwrap_or(Value::Undefined);
    let transfer = transfer_list(ctx, args.get(1))?;
    let message = ctx.serialize(&value, &transfer)?;
    // A closed channel means the parent dropped the handle; a full queue is an error.
    port(ctx)?.outbox.send(Packet::plain(message))?;
    Ok(Value::Undefined)
}

/// The optional second argument: an array of `ArrayBuffer`s to transfer.
pub(crate) fn transfer_list(
    ctx: &mut dyn CallContext,
    list: Option<&Value>,
) -> Result<Vec<Value>, HostError> {
    let Some(list @ Value::Object(_)) = list else {
        return Ok(Vec::new());
    };
    let len = match ctx.get_property(list, "length")? {
        Value::Number(len) if len.fract() == 0.0 && (0.0..=MAX_TRANSFER_LIST).contains(&len) => len,
        _ => {
            return Err(HostError::TypeError(
                "transfer list must be an array of at most 65536 entries".into(),
            ));
        }
    };
    (0..len as u32)
        .map(|index| ctx.get_property(list, &index.to_string()))
        .collect()
}

fn op_worker_recv_message(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let inbox = port(ctx)?.inbox.clone();
    op_async(ctx, |_token| async move {
        Ok(CompletionValue::Bool(inbox.fill().await == Fill::Ready))
    })
}

fn op_worker_take_message(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let inbox = port(ctx)?.inbox.clone();
    // Error packets only matter to the parent; skip anything that is not a message.
    while let Some(packet) = inbox.pop() {
        if let Packet::Message(message) = packet {
            return ctx.deserialize(&message.value);
        }
    }
    Ok(Value::Undefined)
}

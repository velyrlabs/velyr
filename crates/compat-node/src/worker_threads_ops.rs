//! Host op behind `node:worker_threads`: `op_node_worker(kind, arg)`, the synchronous channel that
//! hands `workerData` to a worker. A worker's JS reads `workerData` while its entry module is still
//! loading, before any message can arrive, so the parent parks the structured-clone bytes here and
//! the worker takes them once.
//!
//! - `"park"` (arg: `{ workerData, environmentData }`) serializes in the parent and returns
//!   `[threadId, key]`. The key (thread id plus a random nonce) travels in the web `Worker` name.
//! - `"take"` (arg: key) removes the entry and returns `[threadId, value]` in the worker's isolate,
//!   or `undefined` when there is none (a worker started by the plain web API).
//! - `"discard"` (arg: key) drops an entry the worker never took (spawn failed, worker gone).
//!
//! shortcut: parked entries are capped at `MAX_PARKED`; a parent that spawns faster than workers
//! start gets an error instead of unbounded memory. Raise it if real workloads hit it.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, SerializedValue, Value};

pub(crate) const WORKER_FACTORIES: &[(&str, &str)] =
    &[("worker_threads", include_str!("../js/worker_threads.js"))];

pub(crate) const WORKER_EXPORTS: &[&str] = &[
    "BroadcastChannel",
    "MessageChannel",
    "MessagePort",
    "SHARE_ENV",
    "Worker",
    "getEnvironmentData",
    "isMainThread",
    "markAsUncloneable",
    "parentPort",
    "receiveMessageOnPort",
    "resourceLimits",
    "setEnvironmentData",
    "threadId",
    "workerData",
];

const MAX_PARKED: usize = 1024;

struct Parked {
    thread_id: u64,
    value: SerializedValue,
}

static PARKED: Mutex<Option<HashMap<String, Parked>>> = Mutex::new(None);
// The main thread is 0; workers count up from 1, across every isolate of the process.
static NEXT_THREAD: AtomicU64 = AtomicU64::new(1);

pub(crate) fn worker_threads_host_functions() -> Vec<HostFunctionDef> {
    vec![HostFunctionDef {
        name: "op_node_worker",
        arity: 2,
        callback: op_worker,
    }]
}

fn parked() -> MutexGuard<'static, Option<HashMap<String, Parked>>> {
    PARKED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn nonce() -> u64 {
    RandomState::new().build_hasher().finish()
}

fn op_worker(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(kind)) = args.first() else {
        return Err(HostError::TypeError(
            "op_node_worker expects (kind, arg)".into(),
        ));
    };
    let arg = args.get(1).cloned().unwrap_or(Value::Undefined);
    match kind.to_string_lossy().as_str() {
        "park" => park(ctx, &arg),
        "take" => take(ctx, &arg),
        "discard" => {
            if let Value::String(key) = &arg {
                parked()
                    .as_mut()
                    .and_then(|map| map.remove(&key.to_string_lossy()));
            }
            Ok(Value::Undefined)
        }
        _ => Err(HostError::TypeError("unknown op_node_worker kind".into())),
    }
}

fn park(ctx: &mut dyn CallContext, arg: &Value) -> Result<Value, HostError> {
    // Serialize first: an unclonable `workerData` fails here as a DataCloneError.
    let value = ctx.serialize(arg, &[])?;
    let thread_id = NEXT_THREAD.fetch_add(1, Ordering::Relaxed);
    let key = format!("{thread_id}-{:016x}", nonce());
    {
        let mut guard = parked();
        let map = guard.get_or_insert_with(HashMap::new);
        if map.len() >= MAX_PARKED {
            return Err(HostError::TypeError(
                "too many workers are starting at once".into(),
            ));
        }
        map.insert(key.clone(), Parked { thread_id, value });
    }
    let key = ctx.create_string(key.as_str().into());
    #[allow(clippy::cast_precision_loss)]
    let id = Value::Number(thread_id as f64);
    Ok(ctx.create_array(&[id, key]))
}

fn take(ctx: &mut dyn CallContext, arg: &Value) -> Result<Value, HostError> {
    let Value::String(key) = arg else {
        return Ok(Value::Undefined);
    };
    let entry = parked()
        .as_mut()
        .and_then(|map| map.remove(&key.to_string_lossy()));
    let Some(entry) = entry else {
        return Ok(Value::Undefined);
    };
    let value = ctx.deserialize(&entry.value)?;
    #[allow(clippy::cast_precision_loss)]
    let id = Value::Number(entry.thread_id as f64);
    Ok(ctx.create_array(&[id, value]))
}

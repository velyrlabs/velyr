//! The `op_command_*` ops behind `Velyr.Command`. `spawn` is the only op that creates a process and
//! it authorizes first (`ProcessSpawn`, through the gate); the rest address a child by resource id.
//! A child is a `processes` quota unit and a resource: closing it, or the isolate ending, kills it.
//! Pipes are read one chunk per op call, so a slow consumer pushes back on the child.

use std::any::Any;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWriteExt};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{CapabilityKind, OpContext, op_async};
use velyr_runtime_resources::{QuotaClass, Resource, ResourceId, ResourceTable};

use crate::options::{authorize, parse_options};
use crate::process::{
    Exit, LimitedRead, MAX_BUFFER_LIMIT, Process, read_limited, spawn, wait_exit,
};

/// Largest piece one `read` returns.
const MAX_READ: usize = 64 * 1024;

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
    op!(op_command_spawn, 1, ProcessSpawn),
    op!(op_command_read, 3, None),
    op!(op_command_write, 2, None),
    op!(op_command_close_stdin, 1, None),
    op!(op_command_wait, 1, None),
    op!(op_command_kill, 1, None),
    op!(op_command_collect, 3, None),
    op!(op_command_close, 1, None),
];

struct Handle {
    stdin: Mutex<Option<ChildStdin>>,
    stdout: Mutex<Option<ChildStdout>>,
    stderr: Mutex<Option<ChildStderr>>,
    kill: CancellationToken,
    exit: watch::Receiver<Option<Exit>>,
}

struct ProcessRes(Arc<Handle>);

impl Resource for ProcessRes {
    fn kind(&self) -> &'static str {
        "process"
    }

    fn close(self: Box<Self>) {
        self.0.kill.cancel();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn type_error(op: &str, index: usize, what: &str) -> HostError {
    HostError::TypeError(format!("{op} expects argument {index} to be {what}"))
}

fn op_context(ctx: &mut dyn CallContext) -> Result<&mut OpContext, HostError> {
    ctx.isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))
}

fn handle_arg(
    ctx: &mut dyn CallContext,
    args: &[Value],
    op: &str,
) -> Result<Arc<Handle>, HostError> {
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error(op, 0, "a resource id"));
    };
    let id = ResourceId::from_js_number(*id)?;
    Ok(op_context(ctx)?.resources.get::<ProcessRes>(id)?.0.clone())
}

/// Which pipe: 1 is stdout, 2 is stderr.
fn stream_arg(args: &[Value], op: &str) -> Result<u8, HostError> {
    match args.get(1) {
        Some(Value::Number(n)) if *n == 1.0 || *n == 2.0 => Ok(*n as u8),
        _ => Err(type_error(op, 1, "1 (stdout) or 2 (stderr)")),
    }
}

fn spawn_failure(err: &std::io::Error) -> VelyrError {
    let code = if err.kind() == std::io::ErrorKind::NotFound {
        ErrorCode::NotFound
    } else {
        ErrorCode::InvalidData
    };
    VelyrError::new(code, format!("failed to spawn: {err}"))
}

fn io_failure(err: &std::io::Error) -> VelyrError {
    VelyrError::new(ErrorCode::Network, err.to_string())
}

/// `op_command_spawn(optionsJson) -> string`: options `{program, args, cwd?, env, stdin, stdout,
/// stderr}`; returns JSON `{id, pid}`. Needs `process-spawn` for the program; `env` is the whole
/// environment of the child.
pub fn op_command_spawn(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let op = "op_command_spawn";
    let json = match args.first() {
        Some(Value::String(text)) => text.to_string_lossy(),
        _ => return Err(type_error(op, 0, "a JSON options string")),
    };
    let (program, mut spec) = parse_options(&json)?;
    let resolved = authorize(ctx, op, &program, &mut spec)?;
    let process = spawn(&resolved, &spec).map_err(|e| spawn_failure(&e))?;
    let pid = process.pid;
    let id = register(&mut op_context(ctx)?.resources, process)?;
    let json = serde_json::json!({"id": id.to_js_number(), "pid": pid}).to_string();
    Ok(ctx.create_string(json.as_str().into()))
}

/// Puts a freshly spawned child in the resource table. If that fails, nothing would own the child
/// any more, so it is killed rather than left running untracked.
fn register(resources: &mut ResourceTable, process: Process) -> Result<ResourceId, VelyrError> {
    let (kill, exit) = process.handles();
    let handle = Arc::new(Handle {
        stdin: Mutex::new(process.stdin),
        stdout: Mutex::new(process.stdout),
        stderr: Mutex::new(process.stderr),
        kill: kill.clone(),
        exit,
    });
    resources
        .insert(Box::new(ProcessRes(handle)), QuotaClass::Processes, None)
        .inspect_err(|_| kill.cancel())
}

async fn read_some<R: AsyncRead + Unpin>(
    slot: &Mutex<Option<R>>,
    max: usize,
    token: CancellationToken,
) -> Result<CompletionValue, VelyrError> {
    use tokio::io::AsyncReadExt;
    let mut guard = slot.lock().await;
    let stream = guard.as_mut().ok_or_else(VelyrError::bad_resource)?;
    let mut buf = vec![0u8; max];
    let n = tokio::select! {
        () = token.cancelled() => return Err(VelyrError::cancelled()),
        n = stream.read(&mut buf) => n.map_err(|e| io_failure(&e))?,
    };
    if n == 0 {
        return Ok(CompletionValue::Null);
    }
    buf.truncate(n);
    Ok(CompletionValue::Bytes(buf))
}

/// `op_command_read(id, stream, max) -> Promise<ArrayBuffer | null>`: next bytes (at most `max`,
/// capped at 64 KiB) of stdout (1) or stderr (2); `null` at EOF.
pub fn op_command_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let op = "op_command_read";
    let handle = handle_arg(ctx, args, op)?;
    let stream = stream_arg(args, op)?;
    let max = match args.get(2) {
        Some(Value::Number(n)) if *n >= 1.0 => (*n as usize).min(MAX_READ),
        _ => return Err(type_error(op, 2, "a byte count")),
    };
    op_async(ctx, move |token| async move {
        if stream == 1 {
            read_some(&handle.stdout, max, token).await
        } else {
            read_some(&handle.stderr, max, token).await
        }
    })
}

async fn collect_all<R: AsyncRead + Unpin>(
    slot: &Mutex<Option<R>>,
    max: usize,
    kill: &CancellationToken,
    token: CancellationToken,
) -> Result<CompletionValue, VelyrError> {
    let mut guard = slot.lock().await;
    let stream = guard.as_mut().ok_or_else(VelyrError::bad_resource)?;
    let read = tokio::select! {
        () = token.cancelled() => return Err(VelyrError::cancelled()),
        read = read_limited(stream, max) => read.map_err(|e| io_failure(&e))?,
    };
    match read {
        LimitedRead::Done(data) => Ok(CompletionValue::Bytes(data)),
        LimitedRead::Overflow(_) => {
            // The child would block on a full pipe forever: stop it and drop what it sent.
            kill.cancel();
            Err(VelyrError::quota_exceeded("maxBuffer"))
        }
    }
}

/// `op_command_collect(id, stream, maxBuffer) -> Promise<ArrayBuffer>`: reads the stream to EOF.
/// Past `maxBuffer` bytes the child is killed and the promise rejects with `VELYR_QUOTA_EXCEEDED`,
/// so memory stays bounded by `maxBuffer` plus a chunk.
pub fn op_command_collect(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let op = "op_command_collect";
    let handle = handle_arg(ctx, args, op)?;
    let stream = stream_arg(args, op)?;
    let max = match args.get(2) {
        Some(Value::Number(n)) if *n >= 0.0 && *n <= MAX_BUFFER_LIMIT as f64 => *n as usize,
        _ => return Err(type_error(op, 2, "a byte limit of at most 64 MiB")),
    };
    op_async(ctx, move |token| async move {
        if stream == 1 {
            collect_all(&handle.stdout, max, &handle.kill, token).await
        } else {
            collect_all(&handle.stderr, max, &handle.kill, token).await
        }
    })
}

/// `op_command_write(id, data) -> Promise<undefined>`: writes to stdin; completes once the pipe took
/// the bytes, so a slow child pushes back on the writer.
pub fn op_command_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let op = "op_command_write";
    let handle = handle_arg(ctx, args, op)?;
    let data = args
        .get(1)
        .and_then(|value| ctx.read_bytes(value))
        .ok_or_else(|| type_error(op, 1, "an ArrayBuffer or view"))?;
    op_async(ctx, move |token| async move {
        let mut guard = handle.stdin.lock().await;
        let stdin = guard.as_mut().ok_or_else(VelyrError::bad_resource)?;
        tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            done = stdin.write_all(&data) => done.map_err(|e| io_failure(&e))?,
        }
        Ok(CompletionValue::Undefined)
    })
}

/// `op_command_close_stdin(id)`: closes the child's stdin (it sees EOF once queued writes finish).
pub fn op_command_close_stdin(
    ctx: &mut dyn CallContext,
    args: &[Value],
) -> Result<Value, HostError> {
    let handle = handle_arg(ctx, args, "op_command_close_stdin")?;
    if let Ok(mut stdin) = handle.stdin.try_lock() {
        stdin.take();
    } else {
        // A write is waiting for the pipe: close after it.
        tokio::spawn(async move {
            handle.stdin.lock().await.take();
        });
    }
    Ok(Value::Undefined)
}

/// `op_command_wait(id) -> Promise<string>`: JSON `{code, signal}` once the child ended.
pub fn op_command_wait(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let handle = handle_arg(ctx, args, "op_command_wait")?;
    op_async(ctx, move |token| async move {
        let exit = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            exit = wait_exit(handle.exit.clone()) => exit,
        };
        let json = serde_json::json!({"code": exit.code, "signal": exit.signal});
        Ok(CompletionValue::String(json.to_string()))
    })
}

/// `op_command_kill(id)`: SIGKILL. Harmless when the child already ended.
pub fn op_command_kill(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    handle_arg(ctx, args, "op_command_kill")?.kill.cancel();
    Ok(Value::Undefined)
}

/// `op_command_close(id)`: releases the child; one still running is killed.
pub fn op_command_close(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error("op_command_close", 0, "a resource id"));
    };
    let id = ResourceId::from_js_number(*id)?;
    op_context(ctx)?.resources.close(id)?;
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;
    use std::time::Duration;

    use velyr_runtime_permissions::{Capability, CapabilitySet, ProgramPattern};
    use velyr_runtime_resources::{IsolateId, QuotaConfig, QuotaContext};

    use super::*;
    use crate::gate::authorize_spawn;
    use crate::process::{SpawnSpec, StdioMode};

    #[tokio::test(flavor = "current_thread")]
    async fn a_child_that_cannot_be_registered_is_killed() {
        let all = CapabilitySet::builder()
            .allow(Capability::ProcessSpawn(ProgramPattern::new("*")))
            .build();
        let path = Some(OsStr::new("/bin:/usr/bin"));
        let resolved = authorize_spawn(&all, "sleep", Path::new("/"), path).unwrap();
        let spec = SpawnSpec {
            arg0: "sleep".into(),
            args: vec!["7108".into()],
            cwd: None,
            env: vec![],
            stdin: StdioMode::Null,
            stdout: StdioMode::Null,
            stderr: StdioMode::Null,
        };
        let process = spawn(&resolved, &spec).unwrap();
        let (_, exit) = process.handles();
        // A table whose `processes` quota is zero refuses the insert.
        let quotas = QuotaContext::new(QuotaConfig {
            processes: Some(0),
            ..QuotaConfig::default()
        });
        let mut table = ResourceTable::new(IsolateId(0), Arc::new(quotas));
        assert!(register(&mut table, process).is_err());
        let exit = tokio::time::timeout(Duration::from_secs(10), wait_exit(exit))
            .await
            .expect("the unregistered child is still running");
        assert_eq!(exit.signal, Some(9));
    }
}

//! Host ops behind `process`, `process.env` and `setImmediate`.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, TickFlags, Value};
use velyr_runtime_event_loop::{ImmediateHandle, ImmediateId};
use velyr_runtime_ops::{OpContext, authorize};
use velyr_runtime_permissions::Request;

/// What `process.argv`, `process.execPath` and `process.exit` see. Optional: without it the ops
/// fall back to the current executable, no script and an `exit` that only unwinds JS.
#[derive(Clone)]
pub struct ProcessConfig {
    pub exec_path: String,
    pub script: Option<String>,
    pub args: Vec<String>,
    /// Called by `process.exit(code)`; the host should stop the isolate here.
    pub on_exit: Option<Arc<dyn Fn(i32) + Send + Sync>>,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            exec_path: std::env::current_exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            script: None,
            args: Vec::new(),
            on_exit: None,
        }
    }
}

/// Slot holding the loop's immediate queue (see `install_process_global`).
pub(crate) struct ImmediateSlot(pub ImmediateHandle);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

macro_rules! op {
    ($name:literal, $arity:literal, $f:ident) => {
        HostFunctionDef {
            name: $name,
            arity: $arity,
            callback: $f,
        }
    };
}

pub(crate) fn process_host_functions() -> Vec<HostFunctionDef> {
    vec![
        op!("op_process_info", 0, op_process_info),
        op!("op_process_env_get", 1, op_env_get),
        op!("op_process_env_has", 1, op_env_has),
        op!("op_process_env_set", 2, op_env_set),
        op!("op_process_env_delete", 1, op_env_delete),
        op!("op_process_env_keys", 0, op_env_keys),
        op!("op_process_exit", 1, op_exit),
        op!("op_process_cwd", 0, op_cwd),
        op!("op_process_chdir", 1, op_chdir),
        op!("op_process_hrtime", 0, op_hrtime),
        op!("op_process_uptime", 0, op_uptime),
        op!("op_process_memory", 0, op_memory),
        op!("op_process_write", 2, op_write),
        op!("op_immediate_set", 0, op_immediate_set),
        op!("op_immediate_clear", 1, op_immediate_clear),
        op!("op_immediate_ref", 2, op_immediate_ref),
        op!("op_process_tick_flags", 0, op_tick_flags),
    ]
}

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
        Some(Value::Number(n)) => Ok(*n),
        _ => Err(HostError::TypeError(format!(
            "{op} expects argument {index} to be a number"
        ))),
    }
}

fn set(
    ctx: &mut dyn CallContext,
    object: &Value,
    key: &str,
    value: Value,
) -> Result<(), HostError> {
    ctx.set_property(object, key, value)
}

fn text(ctx: &mut dyn CallContext, value: &str) -> Value {
    ctx.create_string(value.into())
}

fn op_process_info(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    epoch();
    let config = ctx
        .isolate_slot::<ProcessConfig>()
        .cloned()
        .unwrap_or_default();
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let args: Vec<Value> = config.args.iter().map(|a| text(ctx, a)).collect();
    let args = ctx.create_array(&args);
    let info = ctx.create_object();
    let fields = [
        ("platform", text(ctx, platform)),
        ("arch", text(ctx, arch)),
        ("execPath", text(ctx, &config.exec_path)),
        ("velyr", text(ctx, env!("CARGO_PKG_VERSION"))),
        ("pid", Value::Number(std::process::id().into())),
        ("args", args),
    ];
    for (key, value) in fields {
        set(ctx, &info, key, value)?;
    }
    if let Some(script) = &config.script {
        let script = text(ctx, script);
        set(ctx, &info, "script", script)?;
    }
    Ok(info)
}

/// Overlay first, then the process environment; non-Unicode values read as absent.
fn lookup(ctx: &mut dyn CallContext, name: &str) -> Result<Option<String>, HostError> {
    let op_ctx = op_context(ctx)?;
    Ok(op_ctx
        .env_overlay
        .get(name)
        .cloned()
        .or_else(|| std::env::var(name).ok()))
}

fn op_context(ctx: &mut dyn CallContext) -> Result<&mut OpContext, HostError> {
    ctx.isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))
}

fn op_env_get(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let name = string_arg(args, 0, "op_process_env_get")?;
    authorize(ctx, "op_process_env_get", Request::EnvRead(&name))?;
    Ok(match lookup(ctx, &name)? {
        Some(value) => text(ctx, &value),
        None => Value::Undefined,
    })
}

fn op_env_has(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let name = string_arg(args, 0, "op_process_env_has")?;
    authorize(ctx, "op_process_env_has", Request::EnvRead(&name))?;
    Ok(Value::Bool(lookup(ctx, &name)?.is_some()))
}

fn op_env_set(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let name = string_arg(args, 0, "op_process_env_set")?;
    authorize(ctx, "op_process_env_set", Request::EnvWrite(&name))?;
    let value = string_arg(args, 1, "op_process_env_set")?;
    if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
        return Err(HostError::TypeError("invalid environment variable".into()));
    }
    op_context(ctx)?.env_overlay.insert(name, value);
    Ok(Value::Undefined)
}

/// Drops the overlay entry; the JS side hides the inherited value (the real environment is
/// never mutated).
fn op_env_delete(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let name = string_arg(args, 0, "op_process_env_delete")?;
    authorize(ctx, "op_process_env_delete", Request::EnvWrite(&name))?;
    op_context(ctx)?.env_overlay.remove(&name);
    Ok(Value::Undefined)
}

/// Names the isolate may read: filtered silently, so enumeration never throws or logs.
fn op_env_keys(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let op_ctx = op_context(ctx)?;
    let mut names: Vec<String> = std::env::vars_os()
        .filter_map(|(k, v)| v.into_string().ok().and(k.into_string().ok()))
        .chain(op_ctx.env_overlay.keys().cloned())
        .collect();
    names.sort();
    names.dedup();
    names.retain(|n| op_ctx.permissions.check(&Request::EnvRead(n)).is_allow());
    let values: Vec<Value> = names.iter().map(|n| text(ctx, n)).collect();
    Ok(ctx.create_array(&values))
}

fn op_exit(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let code = number_arg(args, 0, "op_process_exit")? as i32;
    let on_exit = ctx
        .isolate_slot::<ProcessConfig>()
        .and_then(|c| c.on_exit.clone());
    if let Some(on_exit) = on_exit {
        on_exit(code);
    }
    Ok(Value::Undefined)
}

fn op_cwd(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let cwd = std::env::current_dir().map_err(|e| HostError::TypeError(e.to_string()))?;
    Ok(text(ctx, &cwd.to_string_lossy()))
}

/// Returns `undefined` on success or the OS error text. Grants nothing: the permission model
/// works on canonical paths, so the working directory does not widen any grant.
fn op_chdir(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let dir = PathBuf::from(string_arg(args, 0, "op_process_chdir")?);
    Ok(match std::env::set_current_dir(dir) {
        Ok(()) => Value::Undefined,
        Err(e) => {
            let code = if e.kind() == std::io::ErrorKind::NotFound {
                "ENOENT"
            } else {
                "EACCES"
            };
            text(ctx, code)
        }
    })
}

fn op_hrtime(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let elapsed = epoch().elapsed();
    let parts = [
        Value::Number(elapsed.as_secs() as f64),
        Value::Number(elapsed.subsec_nanos().into()),
    ];
    Ok(ctx.create_array(&parts))
}

fn op_uptime(_: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Ok(Value::Number(epoch().elapsed().as_secs_f64()))
}

/// `[rss, heapTotal, heapUsed]`; the heap figures are 0 (no engine-neutral source for them).
fn op_memory(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let rss = memory_stats::memory_stats().map_or(0.0, |m| m.physical_mem as f64);
    Ok(ctx.create_array(&[Value::Number(rss), Value::Number(0.0), Value::Number(0.0)]))
}

/// `op_process_write(fd, string | bytes)`: real stdout/stderr; a closed pipe is ignored.
fn op_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let fd = number_arg(args, 0, "op_process_write")?;
    let bytes = match args.get(1) {
        Some(Value::String(s)) => s.to_string_lossy().into_bytes(),
        Some(other) => ctx.read_bytes(other).ok_or_else(|| {
            HostError::TypeError("op_process_write expects string or bytes".into())
        })?,
        None => {
            return Err(HostError::TypeError(
                "op_process_write expects a chunk".into(),
            ));
        }
    };
    let _ = match fd as i32 {
        1 => std::io::stdout().write_all(&bytes),
        2 => std::io::stderr().write_all(&bytes),
        _ => {
            return Err(HostError::TypeError(
                "op_process_write: fd must be 1 or 2".into(),
            ));
        }
    };
    Ok(Value::Undefined)
}

fn immediates(ctx: &mut dyn CallContext) -> Result<ImmediateHandle, HostError> {
    ctx.isolate_slot::<ImmediateSlot>()
        .map(|slot| slot.0.clone())
        .ok_or_else(|| HostError::TypeError("setImmediate needs install_process_global".into()))
}

fn op_immediate_set(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Ok(Value::Number(immediates(ctx)?.set().0 as f64))
}

fn op_immediate_clear(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = number_arg(args, 0, "op_immediate_clear")? as u64;
    immediates(ctx)?.clear(ImmediateId(id));
    Ok(Value::Undefined)
}

fn op_immediate_ref(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = number_arg(args, 0, "op_immediate_ref")? as u64;
    let refd = matches!(args.get(1), Some(Value::Bool(true)));
    immediates(ctx)?.set_ref(ImmediateId(id), refd);
    Ok(Value::Undefined)
}

/// The shared tick flags as a `Uint8Array`, or `undefined` when the host has none to share.
fn op_tick_flags(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let Some(flags) = ctx.isolate_slot::<TickFlags>().cloned() else {
        return Ok(Value::Undefined);
    };
    Ok(ctx.bind_tick_flags(&flags).unwrap_or(Value::Undefined))
}

//! Host op behind the synchronous half of `node:child_process` (`spawnSync`, `execSync`,
//! `execFileSync`, `execFile`'s sync cousins): `op_node_child_run_sync(options, input, limits)`.
//!
//! `options` is the JSON io-process parses (`program`, `args`, `cwd`, `env`, stdio modes), `input` a
//! typed array fed to a piped stdin, `limits` the text `"<timeoutMs>,<maxBuffer>"` (`0` timeout =
//! none). It authorizes through io-process' gate like every spawn (name or resolved path, no shell),
//! runs the child to completion on the calling thread and answers one buffer:
//! `u32 BE json_len | json {code, signal, timedOut, overflowed, stdoutLen} | stdout | stderr`.
//! The async half needs no op here: `node:child_process` is built on `Velyr.Command`.

use std::time::Duration;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_io_process::{MAX_BUFFER_LIMIT, authorize, parse_options, run_blocking};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_ops::OpContext;
use velyr_runtime_resources::QuotaClass;

pub(crate) const CHILD_FACTORIES: &[(&str, &str)] =
    &[("child_process", include_str!("../js/child_process.js"))];

pub(crate) const CHILD_EXPORTS: &[&str] = &[
    "ChildProcess",
    "exec",
    "execFile",
    "execFileSync",
    "execSync",
    "fork",
    "spawn",
    "spawnSync",
];

pub(crate) fn child_process_host_functions() -> Vec<HostFunctionDef> {
    vec![HostFunctionDef {
        name: "op_node_child_run_sync",
        arity: 3,
        callback: op_run_sync,
    }]
}

fn limits(text: &str) -> Option<(Option<Duration>, usize)> {
    let (timeout, max) = text.split_once(',')?;
    let timeout: u64 = timeout.parse().ok()?;
    let max: usize = max.parse().ok().filter(|max| *max <= MAX_BUFFER_LIMIT)?;
    Some(((timeout > 0).then(|| Duration::from_millis(timeout)), max))
}

fn op_run_sync(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let usage = |what: &str| HostError::TypeError(format!("op_node_child_run_sync expects {what}"));
    let (Some(Value::String(options)), Some(input), Some(Value::String(limit))) =
        (args.first(), args.get(1), args.get(2))
    else {
        return Err(usage("(options string, input, limits string)"));
    };
    let (timeout, max_buffer) = limits(&limit.to_string_lossy())
        .ok_or_else(|| usage("limits as \"<timeoutMs>,<maxBuffer>\""))?;
    let input = ctx
        .read_bytes(input)
        .ok_or_else(|| usage("input to be a typed array"))?;
    let (program, mut spec) = parse_options(&options.to_string_lossy())?;
    // Authorize before anything else, then hold a `processes` unit while the child runs.
    let resolved = authorize(ctx, "op_node_child_run_sync", &program, &mut spec)?;
    let op_ctx = ctx
        .isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))?;
    let _unit = op_ctx.quotas.reserve(QuotaClass::Processes, 1)?;
    // Polled while waiting: a terminated isolate must not stay blocked behind a long child.
    let sender = op_ctx.sender.clone();
    let cancelled = move || sender.is_shutdown();
    let run = run_blocking(
        &resolved,
        &spec,
        Some(input),
        max_buffer,
        timeout,
        &cancelled,
    )
    .map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            VelyrError::not_found(format!("failed to spawn: {err}"))
        } else {
            VelyrError::internal(format!("failed to spawn: {err}"))
        }
    })?;
    if run.cancelled {
        return Err(VelyrError::cancelled().into());
    }
    let header = serde_json::json!({
        "code": run.exit.code,
        "signal": run.exit.signal,
        "timedOut": run.timed_out,
        "overflowed": run.overflowed,
        "stdoutLen": run.stdout.len(),
    })
    .to_string();
    let mut out = Vec::with_capacity(4 + header.len() + run.stdout.len() + run.stderr.len());
    out.extend_from_slice(&(header.len() as u32).to_be_bytes());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&run.stdout);
    out.extend_from_slice(&run.stderr);
    Ok(ctx.create_array_buffer(&out))
}

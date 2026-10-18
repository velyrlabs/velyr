//! Spawn options as the JS side sends them (JSON), and the authorization step the ops share.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use velyr_engine_api::{CallContext, HostError};
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_ops::OpContext;
use velyr_runtime_permissions::Request;
use velyr_runtime_resources::QuotaClass;

use crate::gate::{GateError, Resolved, authorize_spawn, normalize};
use crate::process::{SpawnSpec, StdioMode};

const MAX_ARGS: usize = 65_536;

fn invalid(message: &str) -> VelyrError {
    VelyrError::invalid_data(format!("invalid spawn options: {message}"))
}

fn no_nul(text: &str, what: &str) -> Result<(), VelyrError> {
    if text.contains('\0') {
        return Err(invalid(&format!("{what} contains a NUL byte")));
    }
    Ok(())
}

/// `LD_*` and `DYLD_*` make the dynamic loader load code of the caller's choosing into whatever
/// program was granted (`LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`), which turns any `--allow-run` into
/// arbitrary execution. Refused for explicit and inherited environments alike; there is no opt-in.
fn refuse_loader_variable(name: &str) -> Result<(), VelyrError> {
    let upper = name.to_ascii_uppercase();
    if upper.starts_with("LD_") || upper.starts_with("DYLD_") {
        return Err(VelyrError::new(
            ErrorCode::PermissionDenied,
            "dynamic loader variables (LD_*, DYLD_*) are not allowed in a child's environment: \
             they can load arbitrary code into a permitted program",
        )
        .with_public_context("kind", "process-spawn"));
    }
    Ok(())
}

fn mode(options: &serde_json::Value, key: &str) -> Result<StdioMode, VelyrError> {
    options[key]
        .as_str()
        .and_then(StdioMode::parse)
        .ok_or_else(|| invalid(&format!("{key} must be \"piped\", \"inherit\" or \"null\"")))
}

/// Env var the `node` shim reads its permission flags from (see velyr-cli `node_shim`).
pub const SHIM_FLAGS_ENV: &str = "VELYR_NODE_SHIM_FLAGS";

static SHIM_FLAGS: OnceLock<String> = OnceLock::new();

/// Isolate id of the host's main isolate; worker isolates get ids from 1 (runtime-workers).
const MAIN_ISOLATE_ID: u64 = 0;

/// Records, once per process, the flags this process runs with; every child gets exactly these
/// in [`SHIM_FLAGS_ENV`], so a child never has more authority than its parent. Unset: children
/// get no such variable.
pub fn set_shim_flags(flags: String) {
    let _ = SHIM_FLAGS.set(flags);
}

/// Replaces whatever the caller put in [`SHIM_FLAGS_ENV`] (explicit or inherited) with `flags`,
/// or removes it when there are none.
pub fn pin_shim_flags(env: &mut Vec<(OsString, OsString)>, flags: Option<&str>) {
    env.retain(|(name, _)| name != SHIM_FLAGS_ENV);
    if let Some(flags) = flags {
        env.push((SHIM_FLAGS_ENV.into(), flags.into()));
    }
}

/// Variables only the runtime itself may set on a velyr child (`velyr run --watch` hands its
/// report file over this way). A script could point one at a file it may not write, so they never
/// pass through a spawn, explicit or inherited.
pub const INTERNAL_ENV: [&str; 1] = ["VELYR_WATCH_REPORT"];

/// Drops the [`INTERNAL_ENV`] variables from `env`.
pub fn strip_internal_env(env: &mut Vec<(OsString, OsString)>) {
    env.retain(|(name, _)| !INTERNAL_ENV.iter().any(|internal| name == internal));
}

/// `{program, args, cwd?, env: [[name, value], ...], stdin, stdout, stderr}` -> the program as
/// named and the spec. The environment is exactly `env`; the host's is never consulted.
pub fn parse_options(json: &str) -> Result<(String, SpawnSpec), VelyrError> {
    let options: serde_json::Value =
        serde_json::from_str(json).map_err(|_| invalid("not a JSON object"))?;
    let program = options["program"]
        .as_str()
        .ok_or_else(|| invalid("program must be a string"))?
        .to_owned();
    no_nul(&program, "program")?;
    let args = options["args"]
        .as_array()
        .ok_or_else(|| invalid("args must be an array"))?;
    if args.len() > MAX_ARGS {
        return Err(invalid("too many arguments"));
    }
    let args = args
        .iter()
        .map(|arg| {
            let arg = arg
                .as_str()
                .ok_or_else(|| invalid("args must be strings"))?;
            no_nul(arg, "an argument")?;
            Ok(arg.to_owned())
        })
        .collect::<Result<Vec<_>, VelyrError>>()?;
    let mut env = options["env"]
        .as_array()
        .ok_or_else(|| invalid("env must be an array of pairs"))?
        .iter()
        .map(|pair| {
            let (name, value) = (pair[0].as_str(), pair[1].as_str());
            let (Some(name), Some(value)) = (name, value) else {
                return Err(invalid("env entries must be [name, value] strings"));
            };
            if name.is_empty() || name.contains('=') {
                return Err(invalid("invalid environment variable name"));
            }
            no_nul(name, "an environment name")?;
            no_nul(value, "an environment value")?;
            refuse_loader_variable(name)?;
            Ok((OsString::from(name), OsString::from(value)))
        })
        .collect::<Result<Vec<_>, VelyrError>>()?;
    pin_shim_flags(&mut env, SHIM_FLAGS.get().map(String::as_str));
    strip_internal_env(&mut env);
    let cwd = match options.get("cwd").filter(|cwd| !cwd.is_null()) {
        None => None,
        Some(cwd) => {
            let cwd = cwd
                .as_str()
                .ok_or_else(|| invalid("cwd must be a string"))?;
            no_nul(cwd, "cwd")?;
            Some(PathBuf::from(cwd))
        }
    };
    let spec = SpawnSpec {
        arg0: program.clone(),
        args,
        cwd,
        env,
        stdin: mode(&options, "stdin")?,
        stdout: mode(&options, "stdout")?,
        stderr: mode(&options, "stderr")?,
    };
    Ok((program, spec))
}

/// Authorizes `program` for `op` (resolving it on the host `PATH`), makes `spec.cwd` absolute, and
/// checks the `processes` quota has room. Denials are logged like every other permission denial,
/// with the length of the target but never the target.
pub fn authorize(
    ctx: &mut dyn CallContext,
    op: &'static str,
    program: &str,
    spec: &mut SpawnSpec,
) -> Result<Resolved, HostError> {
    let op_ctx = ctx
        .isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))?;
    if op_ctx.isolate_id != MAIN_ISOLATE_ID {
        // The recorded flags are the whole process's; a worker has at most the intersection with
        // them, so its children get none and fall back to the shim's deny default.
        pin_shim_flags(&mut spec.env, None);
    }
    let process_cwd = std::env::current_dir()
        .map_err(|e| VelyrError::internal(format!("no current directory: {e}")))?;
    let cwd: PathBuf = match &spec.cwd {
        Some(cwd) => process_cwd.join(cwd),
        None => process_cwd,
    };
    if spec.cwd.is_some() {
        // A working directory is a read of that path: without this, `cwd` would tell a script
        // which directories exist outside its grants. Decided before anything looks at the program
        // or the directory's existence beyond canonicalizing it.
        let target = dunce::canonicalize(&cwd).unwrap_or_else(|_| normalize(&cwd));
        let text = target
            .to_str()
            .ok_or_else(|| VelyrError::permission_denied("fs-read", "<redacted>"))?;
        op_ctx.authorize(op, Request::FsRead(text))?;
        spec.cwd = Some(target);
    }
    let path_var = std::env::var_os("PATH");
    let resolved = authorize_spawn(
        &op_ctx.permissions,
        program,
        Path::new(&cwd),
        path_var.as_deref(),
    )
    .map_err(|err| match err {
        GateError::Denied => {
            let redacted = format!("<redacted len={}>", program.len());
            tracing::warn!(
                target: "velyr.permission.denied",
                op,
                kind = "process-spawn",
                target_redacted = %redacted,
                isolate_id = op_ctx.isolate_id,
                "velyr.permission.denied"
            );
            VelyrError::permission_denied("process-spawn", &redacted)
        }
        GateError::RuntimeExecutable => {
            tracing::warn!(
                target: "velyr.permission.denied",
                op,
                kind = "process-spawn",
                isolate_id = op_ctx.isolate_id,
                "velyr.permission.denied: the runtime's own executable"
            );
            VelyrError::new(
                ErrorCode::PermissionDenied,
                "spawning the runtime's own executable is not allowed, even when granted: the \
                 child would not inherit this process's permissions",
            )
            .with_public_context("kind", "process-spawn")
        }
        GateError::NotFound => VelyrError::not_found("program not found"),
        GateError::Invalid => invalid("program must be a non-empty string"),
    })?;
    // Probe only: the resource table reserves the unit when the child is registered.
    drop(op_ctx.quotas.reserve(QuotaClass::Processes, 1)?);
    Ok(resolved)
}

//! Child processes (ADR-010 shape): the `op_command_*` ops behind `Velyr.Command`, and the gate and
//! spawn building blocks `node:child_process` is made of.
//!
//! Every process starts through [`authorize_spawn`], which resolves the program on the host `PATH`
//! and checks `ProcessSpawn` by name and by resolved path; [`spawn`] and [`run_blocking`] accept
//! only the [`Resolved`] it returns and run exactly that path with an explicit environment (nothing
//! is inherited), and a spawned child is killed when its owner goes away.

mod blocking;
mod gate;
mod ops;
mod options;
mod process;

pub use blocking::{BlockingOutput, run_blocking};
pub use gate::{GateError, Resolved, authorize_spawn};
pub use ops::{
    op_command_close, op_command_close_stdin, op_command_collect, op_command_kill,
    op_command_read, op_command_spawn, op_command_wait, op_command_write,
};
pub use options::{
    INTERNAL_ENV, SHIM_FLAGS_ENV, authorize, parse_options, pin_shim_flags, set_shim_flags,
    strip_internal_env,
};
pub use process::{
    Exit, LimitedRead, MAX_BUFFER_LIMIT, Process, SpawnSpec, StdioMode, read_limited, spawn,
    wait_exit,
};

use velyr_engine_api::{HostFunctionDef, Isolate, JsException, Source};
use velyr_runtime_ops::CapabilityKind;

/// Bootstrap script: adds `Velyr.Command` on top of the private `op_command_*` bindings.
pub const COMMAND_JS: &str = include_str!("command.js");

/// The process ops with the capability each declares, for the registry audit.
pub fn process_ops() -> Vec<(HostFunctionDef, CapabilityKind)> {
    ops::OPS.to_vec()
}

pub fn process_host_functions() -> Vec<HostFunctionDef> {
    process_ops().into_iter().map(|(def, _)| def).collect()
}

/// Runs [`COMMAND_JS`]. Call after `install_op_context` and after `install_web_streams`
/// (`Child.stdout` and friends use its native stream hooks).
pub fn install_command(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate
        .eval_script(&Source {
            name: "velyr:command".into(),
            text: COMMAND_JS.into(),
        })
        .map(drop)
}

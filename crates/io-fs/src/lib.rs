//! Filesystem host service (ADR-010): the `op_fs_*` ops behind `Velyr.readFile`, `readTextFile`,
//! `writeFile`, `writeTextFile` and `stat`. Every op authorizes the canonical path and then opens
//! that same path; the I/O itself runs on Tokio's blocking pool.

mod ops;
pub mod sync_ops;

use velyr_engine_api::{HostFunctionDef, Isolate, JsException, Source};
use velyr_runtime_ops::CapabilityKind;

/// Bootstrap script: adds the file APIs to `Velyr` on top of the private `op_fs_*` bindings.
pub const FS_JS: &str = include_str!("fs.js");

/// The fs ops with the capability each declares, for the registry audit.
pub fn fs_ops() -> Vec<(HostFunctionDef, CapabilityKind)> {
    ops::OPS.to_vec()
}

pub fn fs_host_functions() -> Vec<HostFunctionDef> {
    fs_ops().into_iter().map(|(def, _)| def).collect()
}

/// Runs [`FS_JS`]. Call from the `install` hook of `run_file`, after `install_op_context` and
/// after the `fs_host_functions` are registered.
pub fn install_fs(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate
        .eval_script(&Source {
            name: "velyr:fs".into(),
            text: FS_JS.into(),
        })
        .map(drop)
}

//! Host operations (ADR-010): named Rust functions JS bootstrap code calls through a private
//! binding. Ops are named `op_<area>_<verb>`; user code never sees them.
//!
//! Binding contract: the engine installs every op as a global named after the op, the embedded
//! [`BOOTSTRAP_JS`] captures the ones it needs in a closure and `delete`s them from the global.

mod async_op;
mod console;
mod context;
mod env;
mod permissions_query;
mod registry;
mod test_ops;

pub use async_op::op_async;
pub use context::{OpContext, authorize, install_op_context, take_leak_report};
pub use registry::{CapabilityKind, OpId, OpRegistry};
pub use test_ops::{test_host_functions, test_ops};

/// Bootstrap script, run once per isolate: `console` and the `Velyr` namespace (bootstrap.js),
/// then `Velyr.env` and `Velyr.permissions`. Each part is its own closure that captures its ops
/// and deletes them from the global.
pub const BOOTSTRAP_JS: &str = concat!(
    include_str!("bootstrap.js"),
    "\n",
    include_str!("../js/env.js"),
    "\n",
    include_str!("../js/permissions.js"),
);

use velyr_runtime_core::RunError;
use velyr_runtime_errors::VelyrError;

/// Everything an embedded run can fail with.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The host op registry rejected a name (not `op_<area>_<verb>`, or already taken).
    #[error("invalid host op: {0}")]
    Op(VelyrError),
    /// The module path or URL is not usable.
    #[error("invalid module `{0}`")]
    InvalidModule(String),
    /// [`Runtime::call`](crate::Runtime::call) before a successful `run_module`.
    #[error("no module loaded: call run_module first")]
    NoModule,
    /// The run itself failed: uncaught error, denied permission, exceeded quota, ...
    #[error("{0}")]
    Run(RunError),
    /// The export finished without returning a JSON-serializable value.
    #[error("the export returned no JSON value")]
    NoResult,
    /// Scratch directory or wrapper file I/O.
    #[error("embed io: {0}")]
    Io(#[from] std::io::Error),
}

//! Engine-generic runtime skeleton on top of `velyr-engine-api`.
//!
//! Lifecycle (see `docs/architecture/runtime-core.md`):
//!
//! 1. create runtime: [`Runtime::new`] runs the process-wide platform init once (ADR-010);
//! 2. create isolate: [`Runtime::create_isolate`] builds an engine isolate and bootstraps it
//!    (host functions and bootstrap JS arrive through [`RuntimeIsolate::bootstrap`]);
//! 3. eval entry: [`RuntimeIsolate::eval`] runs a classic script;
//! 4. shutdown: [`RuntimeIsolate::shutdown`] closes resources and destroys the engine isolate.
//!
//! A [`RuntimeIsolate`] is `!Send` (it wraps an `Isolate`) and must stay on its creating thread.

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Once};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use velyr_engine_api::{
    Engine, EngineError, HostFunctionDef, Isolate, IsolateOptions, JsException, Source,
    TerminationHandle, Value,
};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{ModuleLoader, ModuleResolver, SourceTransform};
use velyr_runtime_permissions::CapabilitySet;
use velyr_runtime_resources::{LeakEntry, QuotaConfig};
use watchdog::Watchdog;

mod blocking_pool;
mod esm;
mod event_loop;
pub mod metrics;
mod shutdown;
mod watchdog;

pub use blocking_pool::peak_blocking_threads;
pub use esm::{EsmRequire, require_esm};
pub use event_loop::{LoopIsolate, run_file, run_module_file, run_module_file_on_current_thread};

static PLATFORM_INIT: Once = Once::new();

/// Process-level configuration.
#[derive(Clone)]
pub struct RuntimeConfig {
    /// Completions processed per event-loop tick (ADR-010).
    pub max_completions_per_tick: usize,
    /// Global engine setup (e.g. the V8 platform). Runs at most once per process, on the
    /// first [`Runtime::new`]; later callbacks are ignored.
    pub platform_init: Option<fn()>,
    /// Granted authority, assembled by the host. Hosts pass it to `install_op_context`, which
    /// enforces it at the op boundary; `None` means deny-all (`Arc::default()`).
    pub capabilities: Option<Arc<CapabilitySet>>,
    /// Resource limits, assembled by the host; hosts pass them to `install_op_context`.
    pub quotas: Option<QuotaConfig>,
    /// Hard JS heap limit; hitting it ends the run with [`RunError::QuotaExceeded`] (`heap`).
    pub heap_limit_bytes: Option<usize>,
    /// Wall-clock budget of one synchronous execution; exceeding it ends the run with
    /// [`RunError::QuotaExceeded`] (`cpu`).
    pub cpu_budget_ms: Option<u64>,
    /// Fails an otherwise successful run with [`RunError::Leak`] when resources are still open
    /// at isolate teardown. The host decides (the CLI reads `VELYR_LEAK_CHECK`); core never
    /// reads the environment.
    pub leak_check: bool,
    /// Drains the leak report of the isolate that just shut down. Called on the isolate thread
    /// after the isolate is dropped; hosts pass `velyr_runtime_ops::take_leak_report`.
    pub leak_collector: Option<fn() -> Vec<LeakEntry>>,
    /// Cancelling it stops the event loop, discards late completions and closes resources;
    /// the run then ends with [`RunError::Terminated`]. Pair it with the isolate's
    /// [`TerminationHandle`] to also interrupt running JS.
    pub shutdown: Option<CancellationToken>,
    /// Resolves import specifiers of the entry module's graph; `None` is [`FsResolver`]
    /// (relative, absolute and `file:` specifiers only). Hosts that support bare specifiers
    /// (npm packages, import maps) pass their resolver here.
    pub module_resolver: Option<ModuleResolverHandle>,
    /// Rewrites every module's text after it is read (TypeScript stripping); applies to `import`
    /// and `require(esm)`. `None` loads sources as written.
    pub source_transform: Option<Arc<dyn SourceTransform>>,
    /// Reads module text for the entry module's graph; `None` is `FsLoader` (the disk). Single-file
    /// executables pass a loader over their embedded files.
    pub module_loader: Option<Arc<dyn ModuleLoader>>,
    /// Cap on the isolate's Tokio blocking threads; `None` is `min(8, available_parallelism)`.
    pub max_blocking_threads: Option<usize>,
}

/// A module resolver shared across the isolate thread and its host.
pub type ModuleResolverHandle = Arc<dyn ModuleResolver + Send + Sync>;

impl fmt::Debug for RuntimeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeConfig")
            .field("max_completions_per_tick", &self.max_completions_per_tick)
            .field("capabilities", &self.capabilities)
            .field("quotas", &self.quotas)
            .field("heap_limit_bytes", &self.heap_limit_bytes)
            .field("cpu_budget_ms", &self.cpu_budget_ms)
            .field("leak_check", &self.leak_check)
            .field(
                "module_resolver",
                &self.module_resolver.as_ref().map(|_| "<resolver>"),
            )
            .field(
                "source_transform",
                &self.source_transform.as_ref().map(|_| "<transform>"),
            )
            .finish_non_exhaustive()
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_completions_per_tick: 1024,
            platform_init: None,
            capabilities: None,
            quotas: None,
            heap_limit_bytes: None,
            cpu_budget_ms: None,
            leak_check: false,
            leak_collector: None,
            shutdown: None,
            module_resolver: None,
            source_transform: None,
            module_loader: None,
            max_blocking_threads: None,
        }
    }
}

fn init_once(platform_init: Option<fn()>) {
    PLATFORM_INIT.call_once(|| {
        if let Some(init) = platform_init {
            init();
        }
    });
}

/// Owns process-level configuration and the engine adapter.
pub struct Runtime<E: Engine> {
    engine: E,
    config: RuntimeConfig,
}

impl<E: Engine + Default> Runtime<E> {
    pub fn new(config: RuntimeConfig) -> Self {
        Self::with_engine(E::default(), config)
    }
}

impl<E: Engine> Runtime<E> {
    pub fn with_engine(engine: E, config: RuntimeConfig) -> Self {
        init_once(config.platform_init);
        Self { engine, config }
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    pub fn create_isolate(&self) -> Result<RuntimeIsolate<E>, EngineError> {
        let isolate = self.engine.create_isolate(IsolateOptions {
            heap_limit_bytes: self.config.heap_limit_bytes,
        })?;
        let watchdog = match self.config.cpu_budget_ms {
            Some(ms) => Some(
                Watchdog::spawn(Duration::from_millis(ms), isolate.termination_handle()).map_err(
                    |err| EngineError::from(VelyrError::internal(format!("watchdog: {err}"))),
                )?,
            ),
            None => None,
        };
        tracing::debug!("isolate created");
        Ok(RuntimeIsolate { isolate, watchdog })
    }
}

/// Why [`RuntimeIsolate::run_script`] failed.
#[derive(Debug)]
pub enum RunError {
    /// The script file could not be read.
    Read(VelyrError),
    /// The script threw and nothing caught it.
    Uncaught(JsException),
    /// The isolate was terminated from another thread, or `RuntimeConfig::shutdown` was cancelled.
    Terminated,
    /// The run exceeded a resource quota (`heap` or `cpu`) and was stopped.
    QuotaExceeded { class: &'static str },
    /// A promise was rejected and nothing handled it; carries the rendered reason.
    UnhandledRejection(String),
    /// A module could not be resolved, read or authorized (`VELYR_*` code in the error).
    Load(VelyrError),
    /// The entry module's top-level await never settled and nothing is left to wake it.
    UnsettledEntry,
    /// Resources were still open at teardown (`RuntimeConfig::leak_check`).
    Leak(Vec<LeakEntry>),
    /// The runtime could not start (isolate creation, bootstrap, thread, Tokio).
    Setup(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(err) => err.fmt(f),
            Self::Load(err) => write!(f, "error: {err}"),
            Self::UnsettledEntry => f.write_str(
                "error: top-level await never settled: the event loop has no pending work",
            ),
            Self::Uncaught(exception) => write!(f, "Uncaught {}", exception.message),
            Self::Terminated => f.write_str("execution terminated"),
            Self::QuotaExceeded { class } => VelyrError::quota_exceeded(class).fmt(f),
            Self::UnhandledRejection(reason) => write!(f, "Uncaught (in promise) {reason}"),
            Self::Leak(entries) => {
                write!(
                    f,
                    "error: {} resource(s) left open at shutdown:",
                    entries.len()
                )?;
                entries.iter().try_for_each(|entry| {
                    write!(
                        f,
                        "\n  {} (index {}, generation {})",
                        entry.kind, entry.id.index, entry.id.generation
                    )
                })
            }
            Self::Setup(message) => write!(f, "error: {message}"),
        }
    }
}

impl std::error::Error for RunError {}

/// One engine isolate plus runtime state.
pub struct RuntimeIsolate<E: Engine> {
    isolate: E::Isolate,
    watchdog: Option<Watchdog>,
}

impl<E: Engine> RuntimeIsolate<E> {
    /// Runs `code` as a classic script named `<eval>`.
    ///
    /// Returns [`JsException`] (not `VelyrError`) so the thrown value and stack survive;
    /// convert at the boundary where a `VelyrError` is needed.
    pub fn eval(&mut self, code: &str) -> Result<Value, JsException> {
        self.isolate.eval_script(&Source {
            name: "<eval>".into(),
            text: code.into(),
        })
    }

    /// Registers `host_functions` as globals, then runs `source` as the bootstrap script.
    /// The script is expected to capture and remove the ops it needs (ADR-010).
    pub fn bootstrap(
        &mut self,
        host_functions: &[HostFunctionDef],
        source: &str,
    ) -> Result<(), VelyrError> {
        for def in host_functions {
            self.isolate.register_host_function(*def)?;
        }
        self.isolate
            .eval_script(&Source {
                name: "<bootstrap>".into(),
                text: source.into(),
            })
            .map_err(|exception| {
                VelyrError::internal(format!("bootstrap failed: {}", exception.message))
            })?;
        Ok(())
    }

    /// Reads `path` and runs it as a classic script named after the path, then drains microtasks.
    pub fn run_script(&mut self, path: &Path) -> Result<Value, RunError> {
        eval_file(&mut self.isolate, self.watchdog.as_ref(), path)
    }

    /// Hands the isolate to a new event loop (see [`run_file`]).
    pub(crate) fn into_parts(self) -> (E::Isolate, Option<Watchdog>) {
        (self.isolate, self.watchdog)
    }

    /// `Send + Sync` handle that stops running JS from any thread (watchdogs, supervisor).
    pub fn termination_handle(&self) -> TerminationHandle {
        self.isolate.termination_handle()
    }

    /// Closes runtime resources (none yet) and destroys the engine isolate.
    pub fn shutdown(self) {
        tracing::debug!("isolate shut down");
    }
}

/// Reads `path`, runs it as a classic script named after the path, then drains microtasks.
/// With a `watchdog`, the whole execution counts against the CPU budget.
fn eval_file<I: Isolate>(
    isolate: &mut I,
    watchdog: Option<&Watchdog>,
    path: &Path,
) -> Result<Value, RunError> {
    let text =
        std::fs::read_to_string(path).map_err(|err| RunError::Read(read_error(path, &err)))?;
    let _tick = watchdog.map(Watchdog::begin_tick);
    let value = isolate
        .eval_script(&Source {
            name: path.display().to_string(),
            text,
        })
        .map_err(|exception| run_error(exception, watchdog))?;
    isolate.perform_microtask_checkpoint();
    Ok(value)
}

fn run_error(exception: JsException, watchdog: Option<&Watchdog>) -> RunError {
    let class = match exception.quota_class() {
        Some("heap") => "heap",
        _ if shutdown::is_termination(&exception) && watchdog.is_some_and(Watchdog::fired) => "cpu",
        _ if shutdown::is_termination(&exception) => return RunError::Terminated,
        _ => return RunError::Uncaught(exception),
    };
    quota_exceeded(class)
}

fn quota_exceeded(class: &'static str) -> RunError {
    tracing::warn!(target: "velyr.quota.denied", class, "velyr.quota.denied");
    RunError::QuotaExceeded { class }
}

fn read_error(path: &Path, err: &std::io::Error) -> VelyrError {
    let message = format!("cannot read {}: {err}", path.display());
    match err.kind() {
        std::io::ErrorKind::NotFound => VelyrError::not_found(message),
        _ => VelyrError::internal(message),
    }
}

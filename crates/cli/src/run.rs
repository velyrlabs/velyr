//! `velyr run <file>`: bootstrap an isolate with the built-in ops and run one script.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use velyr_engine_api::{Engine, EngineFeatures, Isolate, Source};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file, run_module_file};
use velyr_runtime_permissions::CapabilitySet;
use velyr_runtime_resources::QuotaConfig;
#[cfg(feature = "web")]
use velyr_web_fetch::ServeInbox;

use crate::permissions::{self, ConfigError, Permissions};
use crate::{config, profiles, transforms, watch_mode};

/// JS engine `velyr run` executes on.
#[derive(Clone, Copy, Default, clap::ValueEnum)]
pub enum EngineKind {
    #[default]
    V8,
    /// Experimental native engine; only in builds with the `engine-native` feature.
    Native,
}

pub fn run(
    file: &Path,
    flags: Permissions,
    engine: EngineKind,
    inspect: Option<crate::inspect::Inspect>,
) -> ExitCode {
    let _span = tracing::info_span!("velyr.cli.run").entered();
    exit_code(prepare(file, flags).and_then(|prepared| match inspect {
        Some(inspect) => crate::inspect::run(file, &prepared, engine, inspect),
        None => run_prepared(file, &prepared, None, Worker::default(), engine),
    }))
}

/// Runs `file`; `after_install` is a script evaluated once the globals are installed, before
/// the entry module runs (`velyr serve` uses it to start the server).
#[cfg(feature = "web")]
pub fn run_with(
    file: &Path,
    flags: Permissions,
    after_install: Option<String>,
    engine: EngineKind,
) -> ExitCode {
    exit_code(execute(file, flags, after_install, engine))
}

fn exit_code(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "web")]
fn execute(
    file: &Path,
    flags: Permissions,
    after_install: Option<String>,
    engine: EngineKind,
) -> Result<(), String> {
    let prepared = prepare(file, flags)?;
    run_prepared(file, &prepared, after_install, Worker::default(), engine)
}

/// The authority and limits a run is built from; several isolates (`velyr serve --workers`)
/// share one.
#[derive(Clone)]
pub struct Prepared {
    pub capabilities: Arc<CapabilitySet>,
    pub quotas: QuotaConfig,
}

/// Loads `velyr.json`, merges the permission flags and prints the warnings.
pub fn prepare(file: &Path, flags: Permissions) -> Result<Prepared, String> {
    // A compiled executable's permissions are baked in; a velyr.json next to the cwd is not its own.
    let loaded = if crate::commands::compile::is_embedded() { None } else { load_config()? };
    let cwd = working_dir()?;
    let built =
        permissions::build(flags, loaded.as_ref(), &cwd, file).map_err(|e| e.to_string())?;
    for warning in &built.warnings {
        eprintln!("{warning}");
    }
    Ok(Prepared {
        capabilities: built.capabilities,
        quotas: built.quotas,
    })
}

pub fn load_config() -> Result<Option<config::Loaded>, String> {
    config::discover(&working_dir()?).map_err(|e| e.to_string())
}

fn working_dir() -> Result<std::path::PathBuf, String> {
    std::env::current_dir().map_err(|e| ConfigError::WorkingDir(e).to_string())
}

/// What makes a run a `velyr serve --workers` worker instead of the whole process.
#[derive(Default)]
pub struct Worker {
    /// Connections the supervisor hands over (see `Velyr.serve({ inherit: true })`).
    #[cfg(feature = "web")]
    pub inbox: Option<ServeInbox>,
    /// Cancelled when the supervisor stops; the run then ends quietly.
    pub shutdown: Option<CancellationToken>,
}

pub fn run_prepared(
    file: &Path,
    prepared: &Prepared,
    after_install: Option<String>,
    worker: Worker,
    engine: EngineKind,
) -> Result<(), String> {
    match engine {
        EngineKind::V8 => {
            let init = Some(V8Engine::init_platform as fn());
            run_on(profiles::v8_engine(), init, file, prepared, after_install, worker)
        }
        #[cfg(feature = "engine-native")]
        EngineKind::Native => {
            let engine = velyr_engine_native::NativeEngine;
            run_on(engine, None, file, prepared, after_install, worker)
        }
        #[cfg(not(feature = "engine-native"))]
        EngineKind::Native => Err("error: --engine native is not compiled in (build velyr-cli with the `engine-native` feature)".into()),
    }
}

pub(crate) fn run_on<E: Engine + Send + 'static>(
    engine: E,
    platform_init: Option<fn()>,
    file: &Path,
    prepared: &Prepared,
    after_install: Option<String>,
    worker: Worker,
) -> Result<(), String> {
    let (permissions, quotas) = (prepared.capabilities.clone(), prepared.quotas.clone());
    let boot = profiles::Boot::of(&engine);
    let config = runtime_config(prepared, platform_init, worker.shutdown, boot.features);
    #[cfg(feature = "web")]
    let mut inbox = worker.inbox;
    let entry = file.to_path_buf();
    let install = move |isolate: &mut _, sender, timers| {
        profiles::install(isolate, sender, timers, permissions, quotas, boot, &entry);
        #[cfg(feature = "web")]
        if let Some(inbox) = inbox.take() {
            isolate.set_slot(inbox);
        }
        if let Some(text) = after_install {
            let source = Source {
                name: "velyr:after-install".into(),
                text,
            };
            isolate
                .eval_script(&source)
                .expect("the after-install script is valid JS");
        }
    };
    let ((functions, bootstrap), path) = (boot.bootstrap(), file.to_path_buf());
    if is_module(file) {
        run_module_file(engine, config, functions, bootstrap, install, path)
    } else {
        run_file(engine, config, functions, bootstrap, install, path)
    }
    .map_err(describe)
}

fn runtime_config(
    prepared: &Prepared,
    platform_init: Option<fn()>,
    shutdown: Option<CancellationToken>,
    features: EngineFeatures,
) -> RuntimeConfig {
    let mut config = RuntimeConfig {
        platform_init,
        capabilities: Some(prepared.capabilities.clone()),
        quotas: Some(prepared.quotas.clone()),
        leak_check: std::env::var_os("VELYR_LEAK_CHECK").is_some_and(|value| value == "1"),
        leak_collector: Some(velyr_runtime_ops::take_leak_report),
        module_resolver: watch_mode::recording(profiles::module_resolver()),
        source_transform: crate::inspect::source_transform(transforms::source_transform(
            &prepared.capabilities,
            features,
        )),
        shutdown,
        ..RuntimeConfig::default()
    };
    crate::commands::compile::install_vfs(&mut config);
    config
}

/// ESM is the canonical entry format (TypeScript and JSX entries are stripped by the module host); other
/// extensions keep the classic-script path.
fn is_module(file: &Path) -> bool {
    matches!(
        file.extension().and_then(|ext| ext.to_str()),
        Some("js" | "mjs" | "cjs" | "ts" | "mts" | "cts" | "tsx" | "jsx")
    )
}

fn describe(err: RunError) -> String {
    match err {
        RunError::Read(err) => format!("error: {err}"),
        RunError::Load(_)
        | RunError::UnsettledEntry
        | RunError::Terminated
        | RunError::QuotaExceeded { .. }
        | RunError::UnhandledRejection(_)
        | RunError::Leak(_)
        | RunError::Setup(_) => err.to_string(),
        RunError::Uncaught(exception) => match exception.stack {
            Some(stack) => format!("Uncaught {}\n{stack}", exception.message),
            None => format!("Uncaught {}", exception.message),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::is_module;
    use std::path::Path;

    #[test]
    fn is_module_accepts_tsx_and_jsx() {
        assert!(is_module(Path::new("a.tsx")));
        assert!(is_module(Path::new("a.jsx")));
    }
}

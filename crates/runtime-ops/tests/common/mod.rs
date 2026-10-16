//! Shared harness: runs a script on V8 with the built-in ops and an explicit capability set.
#![allow(dead_code)]

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::fmt::MakeWriter;
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::CapabilitySet;
use velyr_runtime_resources::QuotaConfig;

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl MakeWriter<'_> for Log {
    type Writer = Log;
    fn make_writer(&self) -> Log {
        self.clone()
    }
}

/// Everything traced so far in this process (the runtime thread has no per-test subscriber).
pub fn events() -> String {
    static LOG: OnceLock<Log> = OnceLock::new();
    let log = LOG.get_or_init(|| {
        let log = Log::default();
        tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_ansi(false)
            .init();
        log
    });
    String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned()
}

/// Runs `script`; it reports by throwing, and the thrown text (or the unhandled rejection reason)
/// comes back.
pub fn run(script: &str, permissions: Arc<CapabilitySet>) -> String {
    events();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.js");
    std::fs::write(&path, script).unwrap();
    let result = run_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            ..RuntimeConfig::default()
        },
        OpRegistry::builtin().host_functions().to_vec(),
        BOOTSTRAP_JS,
        move |isolate, sender, _| {
            install_op_context(isolate, 0, sender, permissions, QuotaConfig::default());
        },
        path,
    );
    match result {
        Err(RunError::Uncaught(exception)) => exception.message,
        Err(RunError::UnhandledRejection(reason)) => reason,
        other => format!("{other:?}"),
    }
}

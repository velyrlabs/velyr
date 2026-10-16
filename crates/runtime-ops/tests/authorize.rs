//! Permission checks at the op boundary, run on V8 through `run_file`.
#![cfg(feature = "test-ops")]

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::fmt::MakeWriter;
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, CapabilityKind, OpRegistry, install_op_context, test_ops};
use velyr_runtime_permissions::{Capability, CapabilitySet, NamePattern};
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

// run_file executes on its own thread, so events go to a process-wide subscriber.
fn events() -> String {
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

/// Runs `script` with `permissions`; the script reports by throwing, which comes back as the message.
fn run(script: &str, permissions: Arc<CapabilitySet>) -> String {
    run_with_bootstrap(BOOTSTRAP_JS, script, permissions)
}

/// Like [`run`], with a custom bootstrap; an empty one leaves every op on the global for the audit.
fn run_with_bootstrap(
    bootstrap: &'static str,
    script: &str,
    permissions: Arc<CapabilitySet>,
) -> String {
    events();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.js");
    std::fs::write(&path, script).unwrap();
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(test_ops().into_iter().map(|(def, _)| def));
    let result = run_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            ..RuntimeConfig::default()
        },
        functions,
        bootstrap,
        move |isolate, sender, _| {
            install_op_context(isolate, 0, sender, permissions, QuotaConfig::default());
        },
        path,
    );
    match result {
        Err(RunError::Uncaught(exception)) => exception.message,
        other => format!("{other:?}"),
    }
}

#[test]
fn denied_env_read_throws_permission_denied_and_emits_event_without_value() {
    // The workspace forbids `unsafe`, so no `set_var`: use a variable cargo already sets.
    let secret = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let secret = secret.as_str();
    let message = run(
        r#"try { nativeEnvRead("CARGO_MANIFEST_DIR"); } catch (e) { throw new Error(e.code + "|" + e.message); }"#,
        CapabilitySet::builder().build(),
    );
    assert!(message.contains("VELYR_PERMISSION_DENIED"), "{message}");
    assert!(!message.contains(secret), "{message}");
    assert!(!message.contains("CARGO_MANIFEST_DIR"), "{message}");

    let log = events();
    let line = log
        .lines()
        .find(|l| l.contains("velyr.permission.denied") && l.contains("nativeEnvRead"))
        .unwrap_or_else(|| panic!("no denial event in:\n{log}"));
    assert!(line.contains("env-read"), "{line}");
    assert!(line.contains("isolate_id=0"), "{line}");
    assert!(!log.contains(secret), "{log}");
    assert!(!log.contains("CARGO_MANIFEST_DIR"), "{log}");
}

#[test]
fn allowed_env_read_returns_the_value() {
    let value = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let permissions = CapabilitySet::builder()
        .allow(Capability::EnvRead(NamePattern::new("CARGO_MANIFEST_DIR")))
        .build();
    let message = run(
        r#"throw new Error("got:" + nativeEnvRead("CARGO_MANIFEST_DIR"));"#,
        permissions,
    );
    assert_eq!(message, format!("Error: got:{value}"));
}

#[test]
fn every_op_that_requires_a_capability_denies_an_empty_set() {
    let mut registry_ops = OpRegistry::builtin().ops();
    registry_ops.extend(test_ops());
    let guarded: Vec<_> = registry_ops
        .iter()
        .filter(|(_, kind)| *kind != CapabilityKind::None)
        .map(|(def, _)| def.name)
        .collect();
    assert!(guarded.contains(&"nativeEnvRead"));
    assert!(guarded.contains(&"op_env_get"));

    let calls: String = guarded
        .iter()
        .map(|name| {
            format!(r#"try {{ {name}("x"); out.push("{name}:ok"); }} catch (e) {{ out.push("{name}:" + e.code); }}"#)
        })
        .collect();
    let message = run_with_bootstrap(
        "",
        &format!(r#"const out = []; {calls} throw new Error(out.join(","));"#),
        CapabilitySet::builder().build(),
    );
    let log = events();
    for name in guarded {
        assert!(
            message.contains(&format!("{name}:VELYR_PERMISSION_DENIED")),
            "{message}"
        );
        assert!(
            log.lines()
                .any(|l| l.contains("velyr.permission.denied") && l.contains(name)),
            "{name} denied without authorize event"
        );
    }
}

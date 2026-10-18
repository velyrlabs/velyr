//! `LoopIsolate` must forward every defaulted `Isolate` method: the trait default would drop
//! the call and the engine would never see it. One test per forwarded method; each probes the
//! effect on a real V8 isolate through `with_loop_isolate`, or counts calls on `common::MockIsolate`
//! when V8 shows no observable difference.

mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use velyr_engine_api::{
    HostError, Isolate, JsString, ModuleResolveHook, SharedSource, Source, Value,
};
use velyr_engine_v8::{V8Engine, V8Isolate};
use velyr_runtime_core::{LoopIsolate, RuntimeConfig, run_file, run_module_file};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

/// Runs `probe` against the `LoopIsolate` that wraps the engine and returns what it produced.
fn with_loop_isolate<T: Send + 'static>(
    probe: impl FnOnce(&mut LoopIsolate<V8Isolate>) -> T + Send + 'static,
) -> T {
    let dir = dunce::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("velyr-forwarding-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.mjs"), "export {};").unwrap();
    let capabilities = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            dir.to_str().unwrap(),
        )))
        .build();
    let config = RuntimeConfig {
        capabilities: Some(capabilities),
        platform_init: Some(V8Engine::init_platform),
        ..RuntimeConfig::default()
    };
    let out = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&out);
    let mut probe = Some(probe);
    run_module_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        move |isolate, _, _| {
            let probe = probe.take().expect("setup runs once");
            *slot.lock().unwrap() = Some(probe(isolate));
        },
        dir.join("main.mjs"),
    )
    .expect("run");
    out.lock().unwrap().take().expect("probe ran")
}

struct NoImports;

impl ModuleResolveHook for NoImports {
    fn resolve(&mut self, specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        Err(HostError::TypeError(format!(
            "unexpected import `{specifier}`"
        )))
    }
}

#[test]
fn loop_isolate_forwards_set_entry_module() {
    let seen = with_loop_isolate(|isolate| {
        isolate.set_entry_module("file:///a/main.mjs");
        let source = Source {
            name: "file:///a/main.mjs".into(),
            text: "globalThis.seen = String(import.meta.main);".into(),
        };
        let module = isolate.compile_module(&source).expect("compile");
        isolate
            .instantiate_module(module, &mut NoImports)
            .expect("link");
        isolate.evaluate_module(module).expect("evaluate");
        let probe = Source {
            name: "probe.js".into(),
            text: "globalThis.seen".into(),
        };
        isolate.eval_script(&probe).expect("probe")
    });
    assert_eq!(seen, Value::String(JsString::from("true")));
}

#[test]
fn loop_isolate_forwards_check_script() {
    let (valid, invalid) = with_loop_isolate(|isolate| {
        let source = |text: &str| Source {
            name: "check.js".into(),
            text: text.into(),
        };
        let valid = isolate.check_script(&source("1 + 1;")).is_ok();
        let invalid = isolate.check_script(&source("let x; let x;")).unwrap_err();
        (valid, invalid.is_unsupported())
    });
    // An unforwarded call would hit the trait default: `Err(unsupported)` for valid input too.
    assert!(valid);
    assert!(!invalid);
}

#[test]
fn loop_isolate_forwards_compile_function_and_function_cache() {
    let (compiled, cached) = with_loop_isolate(|isolate| {
        let source = Source {
            name: "fn.js".into(),
            text: "return a + 1;".into(),
        };
        // The trait defaults return `Err(unsupported)` and `None`.
        let compiled = isolate.compile_function(&source, &["a"], None).is_ok();
        (
            compiled,
            isolate.create_function_code_cache("fn.js").is_some(),
        )
    });
    assert!(compiled);
    assert!(cached);
}

#[test]
fn loop_isolate_forwards_compile_module_shared() {
    let count = with_loop_isolate(|isolate| {
        let text: Arc<str> = Arc::from("export const a = 1;");
        let source = SharedSource {
            name: "file:///shared.mjs".into(),
            text: Arc::clone(&text),
        };
        isolate
            .compile_module_shared(&source, None)
            .expect("compile");
        Arc::strong_count(&text)
    });
    // The caller's two clones plus V8's keepalive; the copy default leaves it at 2.
    assert!(count >= 3, "strong count {count}");
}

#[test]
fn loop_isolate_forwards_create_array_buffer_owned() {
    let dir = std::env::temp_dir().join(format!("velyr-forwarding-owned-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.js"), "0").unwrap();
    run_file(
        common::MockEngine,
        RuntimeConfig::default(),
        Vec::new(),
        "0",
        |isolate, _, _| {
            isolate.create_array_buffer_owned(vec![1, 2, 3]);
        },
        dir.join("main.js"),
    )
    .expect("run");
    // The trait default would copy through `create_array_buffer` and never reach the mock.
    assert_eq!(common::OWNED_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn loop_isolate_forwards_heap_exhausted() {
    const LIMIT: usize = 32 << 20;
    const HEADROOM: usize = 8 << 20;
    let dir = std::env::temp_dir().join(format!("velyr-forwarding-heap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.js"), "0").unwrap();
    let out = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&out);
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        heap_limit_bytes: Some(LIMIT),
        ..RuntimeConfig::default()
    };
    let result = run_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        move |isolate, _, _| {
            let before = isolate.heap_exhausted();
            let initial = isolate.heap_stats().limit_bytes.unwrap();
            let hog = Source {
                name: "hog.js".into(),
                text: "globalThis.keep = []; for (;;) keep.push(new Array(1e5).fill(1));".into(),
            };
            // A second run must not get another headroom grant.
            let errors: Vec<_> = (0..2).map(|_| isolate.eval_script(&hog).is_err()).collect();
            let grown = isolate.heap_stats().limit_bytes.unwrap() - initial;
            *slot.lock().unwrap() = Some((before, isolate.heap_exhausted(), errors, grown));
        },
        dir.join("main.js"),
    );
    let (before, after, errors, grown) = out.lock().unwrap().take().expect("probe ran");
    // The trait default reports `false` forever.
    assert!(!before && after);
    assert_eq!(errors, [true, true]);
    assert!(grown <= HEADROOM, "the limit grew by {grown} bytes");
    let result = result.expect_err("the entry runs on a heap-exhausted isolate");
    assert!(
        result.to_string().contains("heap quota exceeded"),
        "{result}"
    );
}

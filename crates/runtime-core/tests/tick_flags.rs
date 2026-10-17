//! The `nextTick` hook is entered only when a tick is queued (RT-09).

use std::sync::{Arc, Mutex};

use velyr_engine_api::{Isolate, Source, Value};
use velyr_engine_v8::{V8Engine, V8Isolate};
use velyr_runtime_core::{LoopIsolate, RuntimeConfig, run_module_file};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

fn with_loop_isolate<T: Send + 'static>(
    probe: impl FnOnce(&mut LoopIsolate<V8Isolate>) -> T + Send + 'static,
) -> T {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("velyr-tick-flags-{}", std::process::id()));
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
    let value = out.lock().unwrap().take().expect("probe ran");
    value
}

fn eval(isolate: &mut LoopIsolate<V8Isolate>, text: &str) -> Value {
    let source = Source {
        name: "tick-flags.js".into(),
        text: text.into(),
    };
    isolate.eval_script(&source).expect("eval")
}

#[test]
fn tick_hook_skipped_while_flag_clear() {
    let (idle_calls, queued_calls) = with_loop_isolate(|isolate| {
        let hook = eval(
            isolate,
            "globalThis.calls = 0; () => { globalThis.calls++; return false; }",
        );
        isolate.set_tick_hook(hook);
        let flags = isolate.tick_flags();
        flags.arm();
        for _ in 0..1000 {
            isolate.perform_microtask_checkpoint();
        }
        let idle = eval(isolate, "globalThis.calls");
        flags.set_tick();
        isolate.perform_microtask_checkpoint();
        (idle, eval(isolate, "globalThis.calls"))
    });
    match idle_calls {
        Value::Number(n) => assert!(n <= 2.0, "hook entered {n} times with no tick queued"),
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(queued_calls, Value::Number(n) if n > 0.0));
}

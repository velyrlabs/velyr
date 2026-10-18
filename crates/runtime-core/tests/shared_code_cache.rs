//! The shared code cache layer end to end: the first isolate publishes, the next consumes
//! (VELYR-60). One test only: the layer is process-global once enabled.

use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_modules::SharedCache;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

fn run(dir: &std::path::Path) -> Result<(), velyr_runtime_core::RunError> {
    let config = RuntimeConfig {
        capabilities: Some(
            CapabilitySet::builder()
                .allow(Capability::ImportLocal(PathPattern::new(
                    dir.to_str().unwrap(),
                )))
                .build(),
        ),
        platform_init: Some(V8Engine::init_platform),
        ..RuntimeConfig::default()
    };
    run_module_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        |_, _, _| {},
        dir.join("main.mjs"),
    )
}

#[test]
fn second_isolate_consumes_what_the_first_published() {
    let dir = dunce::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("velyr-shared-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.mjs"), "export const a = 40;").unwrap();
    std::fs::write(
        dir.join("b.mjs"),
        "import { a } from './a.mjs'; export const b = a + 2;",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.mjs"),
        "import { b } from './b.mjs'; if (b !== 42) throw new Error('wrong');",
    )
    .unwrap();
    let shared = SharedCache::enable_global();

    run(&dir).expect("first run");
    assert_eq!(shared.uses(), (0, 0), "nothing to consume yet");
    assert!(shared.code_bytes() > 0, "the first isolate publishes");

    run(&dir).expect("second run");
    assert_eq!(shared.uses(), (3, 0), "every module of the graph hits");
}

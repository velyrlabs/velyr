//! `import.meta.main` through `run_module_file`, which wraps the engine in `LoopIsolate`.

use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

#[test]
fn only_the_entry_module_is_main() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("velyr-import-meta-main-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("main.mjs"),
        "import './dep.mjs'; if (import.meta.main !== true) throw new Error('main');",
    )
    .unwrap();
    std::fs::write(
        dir.join("dep.mjs"),
        "if (import.meta.main !== false) throw new Error('dep');",
    )
    .unwrap();
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
    run_module_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        |_, _, _| {},
        dir.join("main.mjs"),
    )
    .expect("entry runs with main true and dependency main false");
}

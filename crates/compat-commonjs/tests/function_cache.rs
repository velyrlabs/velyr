//! A second isolate consumes the function cache the first one published (ML-08). One test
//! only: the shared cache layer is process-global once enabled.

use std::path::Path;

use velyr_compat_commonjs::{commonjs_host_functions, install_commonjs};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_modules::SharedCache;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

fn run(dir: &Path) {
    let permissions = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            dir.to_str().unwrap(),
        )))
        .build();
    let config = RuntimeConfig {
        capabilities: Some(permissions.clone()),
        platform_init: Some(V8Engine::init_platform),
        ..RuntimeConfig::default()
    };
    run_module_file(
        V8Engine::new(),
        config,
        commonjs_host_functions(),
        "",
        move |isolate, _, _| install_commonjs(isolate, permissions).expect("install"),
        dir.join("main.mjs"),
    )
    .expect("run");
}

#[test]
fn cjs_second_isolate_consumes_function_cache() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("velyr-cjs-function-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.cjs"), "module.exports = 40 + 2;").unwrap();
    std::fs::write(
        dir.join("main.mjs"),
        format!(
            "const require = globalThis[Symbol.for('velyr.commonjs')].createRequire({:?});\n\
             if (require('./lib.cjs') !== 42) throw new Error('wrong');",
            dir.join("entry.js").to_str().unwrap()
        ),
    )
    .unwrap();
    let shared = SharedCache::enable_global();

    run(&dir);
    assert_eq!(shared.uses(), (0, 0), "nothing to consume yet");
    assert!(shared.code_bytes() > 0, "the first isolate publishes");

    run(&dir);
    // main.mjs plus the CommonJS function of lib.cjs.
    assert_eq!(shared.uses(), (2, 0));
}

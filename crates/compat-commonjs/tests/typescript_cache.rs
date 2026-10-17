//! The function cache of a `.cts` is keyed by the stripped text (TS-T15): a type-only edit
//! reuses the blob, a runtime edit does not. One test only: the shared cache is process-global.

use std::path::Path;
use std::sync::Arc;

use velyr_compat_commonjs::{commonjs_host_functions, install_commonjs_with};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_modules::SharedCache;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_tooling_typescript::TypeScriptTransform;

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
        move |isolate, _, _| {
            let strip = Arc::new(TypeScriptTransform::new(None));
            install_commonjs_with(isolate, permissions, strip).expect("install");
        },
        dir.join("main.mjs"),
    )
    .expect("run");
}

#[test]
fn cts_code_cache_key_is_the_stripped_text() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("velyr-cts-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("main.mjs"),
        format!(
            "const require = globalThis[Symbol.for('velyr.commonjs')].createRequire({:?});\n\
             if (require('./lib.cts').f(1) !== 1) throw new Error('wrong');",
            dir.join("entry.js").to_str().unwrap()
        ),
    )
    .unwrap();
    let lib = |ty: &str, value: u32| {
        std::fs::write(
            dir.join("lib.cts"),
            format!("export = {{ f(n: {ty}): number {{ return {value}; }} }};\n"),
        )
        .unwrap();
    };
    let shared = SharedCache::enable_global();

    lib("number", 1);
    run(&dir);
    assert_eq!(shared.uses(), (0, 0));

    // Only an annotation changed: the stripped text is identical, so main.mjs and lib.cts hit.
    lib("string", 1);
    run(&dir);
    assert_eq!(shared.uses(), (2, 0));

    // A runtime value changed: lib.cts misses and main.mjs still hits.
    std::fs::write(
        dir.join("lib.cts"),
        "export = { f(n: string): number { return 1 + 0; } };\n",
    )
    .unwrap();
    run(&dir);
    assert_eq!(shared.uses(), (3, 0));
}

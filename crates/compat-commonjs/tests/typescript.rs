//! TypeScript required from CommonJS: `.cts` with `export =` and `import x = require()` loads
//! as CommonJS with its types stripped on the CJS read path (TS-T15, decision D5).

use std::path::Path;
use std::sync::Arc;

use velyr_compat_commonjs::{CommonJsTransform, commonjs_host_functions, install_commonjs_with};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_modules::{MediaType, SourceTransform};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_tooling_typescript::TypeScriptTransform;

const MAIN: &str = "const require = globalThis[Symbol.for('velyr.commonjs')].createRequire(\
    import.meta.url.replace('file://', ''));\n";

fn run(files: &[(&str, &str)]) -> Result<(), String> {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    for (name, text) in files {
        std::fs::write(root.join(name), text).unwrap();
    }
    run_in(&root, "main.mjs")
}

fn run_in(root: &Path, entry: &str) -> Result<(), String> {
    let permissions = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            root.to_str().unwrap(),
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
        root.join(entry),
    )
    .map(drop)
    .map_err(|e| e.to_string())
}

#[test]
fn cts_with_export_equals_and_import_equals_loads_as_commonjs() {
    let main = format!(
        "{MAIN}const m = require('./main.cts');\n\
         if (m.sum !== 42 || m.dir !== 'string') throw new Error(JSON.stringify(m));"
    );
    run(&[
        (
            "lib.cts",
            "const base: number = 40;\nexport = { add(n: number): number { return base + n; } };\n",
        ),
        (
            "main.cts",
            "import lib = require(\"./lib.cjs\");\nexport = { sum: lib.add(2), dir: typeof __dirname };\n",
        ),
        ("main.mjs", &main),
    ])
    .unwrap();
}

#[test]
fn cts_entry_is_a_facade_unless_it_has_real_esm_syntax() {
    let transform = CommonJsTransform::new(Arc::default());
    let facade = |source: &str| {
        transform
            .transform("/p/a.cts", source, MediaType::Cts)
            .unwrap()
            .code
    };
    // Stripped `.cts` output.
    let cjs = "\"use strict\";\nconst v = 1;\nmodule.exports = { v };\n";
    assert!(facade(cjs).contains("importCjs("));
    let esm = "import { add } from \"./m.mjs\";\nexport const x = add(1);\n";
    assert_eq!(facade(esm), esm, "D5: a .cts with ESM syntax loads as ESM");
}

#[test]
fn import_type_alone_does_not_make_a_ts_file_esm() {
    let main = format!("{MAIN}if (require('./t.cts').v !== 1) throw new Error('wrong');");
    run(&[
        (
            "t.cts",
            "import type { X } from './x';\nconst v: number = 1;\nexport = { v };\n",
        ),
        ("main.mjs", &main),
    ])
    .unwrap();
}

#[test]
fn erased_type_import_before_an_export_comment_still_loads_as_commonjs() {
    let main = format!("{MAIN}if (require('./t.cts').v !== 1) throw new Error('wrong');");
    run(&[
        (
            "t.cts",
            "import type { X } from './x';\nconst v: number = 1;\nexport = { v };\n// export {};\n",
        ),
        ("main.mjs", &main),
    ])
    .unwrap();
}

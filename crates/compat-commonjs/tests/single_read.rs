//! `require` reads and classifies each CommonJS file once (resolve keeps the text for the read).

use std::fs;

use velyr_compat_commonjs::{commonjs_host_functions, debug_read_count, install_commonjs};
use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

#[test]
fn require_reads_each_file_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::write(
        root.join("main.js"),
        "module.exports = require('./a') + require('./b');",
    )
    .unwrap();
    fs::write(root.join("a.js"), "module.exports = require('./b') + 1;").unwrap();
    fs::write(root.join("b.js"), "module.exports = 2;").unwrap();
    let permissions = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            root.to_str().unwrap(),
        )))
        .build();
    let mut iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .unwrap();
    for def in commonjs_host_functions() {
        iso.register_host_function(def).unwrap();
    }
    install_commonjs(&mut iso, permissions).unwrap();
    let script = format!(
        "globalThis[Symbol.for('velyr.commonjs')].createRequire({:?})('./main.js')",
        root.join("entry.js").to_str().unwrap()
    );
    let out = iso
        .eval_script(&Source {
            name: "t.js".into(),
            text: script,
        })
        .unwrap();
    assert!(matches!(out, Value::Number(n) if n == 5.0), "{out:?}");
    for file in ["main.js", "a.js", "b.js"] {
        assert_eq!(debug_read_count(&root.join(file)), 1, "{file}");
    }
}

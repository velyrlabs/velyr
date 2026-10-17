//! `package.json` is kept as a typed projection and every manifest read passes the permission
//! check.

use std::fs;
use std::path::Path;

use velyr_compat_commonjs::{commonjs_host_functions, debug_manifest, install_commonjs};
use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

fn require_from(root: &Path, request: &str, permissions: std::sync::Arc<CapabilitySet>) -> String {
    let mut iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .unwrap();
    for def in commonjs_host_functions() {
        iso.register_host_function(def).unwrap();
    }
    install_commonjs(&mut iso, permissions).unwrap();
    let script = format!(
        "try {{ String(globalThis[Symbol.for('velyr.commonjs')].createRequire({:?})({request:?})) }} \
         catch (e) {{ 'ERR:' + String(e && e.message || e) }}",
        root.join("entry.js").to_str().unwrap()
    );
    let out = iso
        .eval_script(&Source {
            name: "t.js".into(),
            text: script,
        })
        .unwrap();
    match out {
        Value::String(text) => text.to_string_lossy(),
        other => panic!("{other:?}"),
    }
}

fn grant(root: &Path) -> std::sync::Arc<CapabilitySet> {
    CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            root.to_str().unwrap(),
        )))
        .build()
}

#[test]
fn manifest_projection_keeps_only_runtime_fields() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::write(
        root.join("package.json"),
        r##"{"name":"leaky-name","version":"1.2.3","type":"commonjs","main":"lib.js",
            "scripts":{"build":"SCRIPTS_MARKER"},"dependencies":{"x":"DEPS_MARKER"},
            "exports":{".":"./lib.js"},"imports":{"#a":"./lib.js"}}"##,
    )
    .unwrap();
    fs::write(root.join("lib.js"), "module.exports = 'lib';").unwrap();
    assert_eq!(require_from(&root, ".", grant(&root)), "lib");
    let kept = debug_manifest(&root).unwrap();
    for field in ["CommonJs", "lib.js", "exports", "imports"] {
        assert!(kept.contains(field), "{field} missing from {kept}");
    }
    for dropped in ["SCRIPTS_MARKER", "DEPS_MARKER", "leaky-name", "1.2.3"] {
        assert!(!kept.contains(dropped), "{dropped} retained in {kept}");
    }
}

#[test]
fn large_scripts_field_is_not_retained() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let blob = "x".repeat(1 << 20);
    fs::write(
        root.join("package.json"),
        format!(r#"{{"main":"lib.js","scripts":{{"big":"{blob}"}}}}"#),
    )
    .unwrap();
    fs::write(root.join("lib.js"), "module.exports = 1;").unwrap();
    assert_eq!(require_from(&root, ".", grant(&root)), "1");
    assert!(debug_manifest(&root).unwrap().len() < 256);
}

#[test]
fn manifest_read_without_grant_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let pkg = root.join("vendor").join("pkg");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(pkg.join("package.json"), r#"{"main":"lib.js"}"#).unwrap();
    fs::write(pkg.join("lib.js"), "module.exports = 'main';").unwrap();
    fs::write(pkg.join("index.js"), "module.exports = 'index';").unwrap();
    assert_eq!(require_from(&root, "./vendor/pkg", grant(&root)), "main");

    let denied = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            root.to_str().unwrap(),
        )))
        .deny(Capability::ImportLocal(PathPattern::new(
            pkg.join("package.json").to_str().unwrap(),
        )))
        .build();
    let out = require_from(&root, "./vendor/pkg", denied);
    assert!(
        out.starts_with("ERR:") && out.contains("import-local"),
        "{out}"
    );
}

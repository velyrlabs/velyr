//! ESM importing CommonJS packages from node_modules under `velyr run`, and the module format
//! following the nearest package.json. The web profile keeps the explicit error.
#![cfg(feature = "npm")]

use std::path::Path;

use assert_cmd::Command;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn run_project(main: &str) -> (assert_cmd::assert::Assert, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "node_modules/cjsfn/package.json",
        r#"{"name":"cjsfn","version":"1.0.0","main":"index.js"}"#,
    );
    write(
        root,
        "node_modules/cjsfn/index.js",
        "module.exports = function answer() { return 42; };\n",
    );
    write(
        root,
        "node_modules/nested/package.json",
        r#"{"name":"nested","version":"1.0.0","main":"index.js"}"#,
    );
    write(
        root,
        "node_modules/nested/index.js",
        "module.exports = 1;\n",
    );
    write(
        root,
        "node_modules/nested/esm/package.json",
        r#"{"type":"module"}"#,
    );
    write(
        root,
        "node_modules/nested/esm/index.js",
        "export default 7;\n",
    );
    write(root, "main.mjs", main);
    let assert = Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(root)
        .args(["run", "-A", "main.mjs"])
        .assert();
    (assert, dir)
}

#[cfg(feature = "node")]
#[test]
fn esm_default_import_of_a_commonjs_package_is_the_function() {
    let (run, _dir) = run_project("import x from 'cjsfn';\nconsole.log(typeof x, x());\n");
    run.success().stdout("function 42\n");
}

#[cfg(feature = "node")]
#[test]
fn the_nearest_package_json_decides_the_format() {
    let (run, _dir) = run_project(
        "import seven from 'nested/esm/index.js';\nimport one from 'nested';\nconsole.log(seven, one);\n",
    );
    run.success().stdout("7 1\n");
}

#[cfg(not(feature = "node"))]
#[test]
fn without_node_compat_commonjs_is_a_clear_error() {
    let (run, _dir) = run_project("import x from 'cjsfn';\nconsole.log(x());\n");
    let out = run.failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(stderr.contains("CommonJS"), "{stderr}");
}

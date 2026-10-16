use std::fs;

use assert_cmd::Command;

fn velyr(args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("velyr").unwrap().args(args).assert()
}

fn run(name: &str) -> assert_cmd::assert::Assert {
    let path = format!(
        "{}/../../tests/fixtures/m3/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    velyr(&["run", &path])
}

fn stderr_of(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stderr).into_owned()
}

#[test]
fn static_import_with_top_level_await() {
    run("tla/main.js")
        .success()
        .stdout("hello velyr\nlib-tla\nafter timer\ntrue\n");
}

#[test]
fn dynamic_import_resolves_namespace_once_and_rejects_with_code() {
    run("dynamic/main.js")
        .success()
        .stdout("start\ndyn evaluated\n42 def\ntrue\nVELYR_NOT_FOUND\n");
}

#[test]
fn import_cycle_follows_esm_semantics() {
    run("cycle/main.js")
        .success()
        .stdout("b sees getA as function\na is in TDZ: ReferenceError\na sees b = b\nmain done\n");
}

#[test]
fn static_import_outside_grant_is_denied_with_exit_1() {
    let assert = run("denied/main.js").code(1).stdout("");
    assert!(stderr_of(&assert).contains("VELYR_PERMISSION_DENIED"));
}

#[test]
fn dynamic_import_outside_grant_rejects_with_permission_code() {
    let assert = run("dynamic-denied/main.js")
        .code(1)
        .stdout("VELYR_PERMISSION_DENIED\n");
    assert!(stderr_of(&assert).contains("VELYR_PERMISSION_DENIED"));
}

#[test]
fn uncaught_module_error_exits_1_after_earlier_output() {
    let assert = run("throws/main.js").code(1).stdout("before\n");
    assert!(stderr_of(&assert).contains("boom"));
}

#[test]
fn classic_globals_are_not_shared_with_module_scope() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::write(
        root.join("main.mjs"),
        "var x = 1; console.log(typeof globalThis.x, this);",
    )
    .unwrap();
    velyr(&["run", root.join("main.mjs").to_str().unwrap()])
        .success()
        .stdout("undefined undefined\n");
}

#[test]
fn import_outside_project_root_needs_allow_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::create_dir_all(root.join("app")).unwrap();
    fs::create_dir_all(root.join("shared")).unwrap();
    fs::write(root.join("shared/x.js"), "export const x = 7;").unwrap();
    fs::write(
        root.join("app/main.js"),
        "import { x } from '../shared/x.js'; console.log(x);",
    )
    .unwrap();
    let main = root.join("app/main.js");
    let main = main.to_str().unwrap();

    let denied = velyr(&["run", main]).code(1);
    assert!(stderr_of(&denied).contains("VELYR_PERMISSION_DENIED"));

    let grant = format!("--allow-read={}", root.join("shared").display());
    velyr(&["run", &grant, main]).success().stdout("7\n");
}

use std::path::PathBuf;
use std::time::Duration;

use assert_cmd::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/npm-resolve")
        .join(name)
}

fn run(name: &str) -> assert_cmd::assert::Assert {
    let root = fixture("");
    Command::cargo_bin("velyr")
        .unwrap()
        .env("VELYR_LEAK_CHECK", "1")
        .timeout(Duration::from_secs(30))
        .arg("run")
        .arg(format!("--allow-read={}", root.display()))
        .arg(fixture(name))
        .assert()
}

#[test]
fn bare_specifier_resolves_through_node_modules_exports() {
    run("main.js").success().stdout("hello from pkg\n");
}

#[test]
fn unexported_subpath_fails_with_not_found() {
    let assert = run("internal.js").failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("VELYR_NOT_FOUND"), "stderr: {stderr}");
}

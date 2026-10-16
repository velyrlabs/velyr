#![cfg(feature = "test-ops")]

use assert_cmd::Command;

fn run(name: &str) -> assert_cmd::assert::Assert {
    let path = format!(
        "{}/../../tests/fixtures/m2/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    Command::cargo_bin("velyr")
        .unwrap()
        .args(["run", &path])
        .assert()
}

#[test]
fn async_op_resolves_after_microtasks() {
    run("async_order.js").success().stdout("a\nmicro\ndone\n");
}

#[test]
fn unhandled_async_rejection_exits_1() {
    let assert = run("async_fail.js").code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("Uncaught (in promise)"), "stderr: {stderr}");
}

#[test]
fn handled_async_rejection_exits_0() {
    run("async_fail_caught.js")
        .success()
        .stdout("caught string\n")
        .stderr("");
}

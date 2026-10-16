#![cfg(feature = "test-ops")]

use assert_cmd::Command;

fn run(leak_check: Option<&str>) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), "nativeLeakOpen();").unwrap();
    let mut cmd = Command::cargo_bin("velyr").unwrap();
    cmd.current_dir(dir.path())
        .env_remove("VELYR_LOG")
        .env_remove("VELYR_LEAK_CHECK")
        .args(["run", "main.js"]);
    if let Some(value) = leak_check {
        cmd.env("VELYR_LEAK_CHECK", value);
    }
    cmd.output().unwrap()
}

#[test]
fn leak_check_fails_and_lists_the_resource_left_open() {
    let output = run(Some("1"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("left open"), "{stderr}");
    assert!(stderr.contains("test_leak"), "{stderr}");
}

#[test]
fn leaked_resource_is_ignored_without_the_flag() {
    let output = run(None);
    assert!(output.status.success());
}

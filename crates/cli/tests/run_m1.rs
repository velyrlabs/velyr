use assert_cmd::Command;

fn fixture(name: &str) -> String {
    format!(
        "{}/../../tests/fixtures/m1/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn run(name: &str) -> assert_cmd::assert::Assert {
    Command::cargo_bin("velyr")
        .unwrap()
        .args(["run", &fixture(name)])
        .assert()
}

#[cfg(feature = "test-ops")]
#[test]
fn native_add_prints_42() {
    run("native_add.js").success().stdout("42\n");
}

#[test]
fn uncaught_error_exits_1_with_stack() {
    let assert = run("throws.js").code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.starts_with("Uncaught "), "stderr: {stderr}");
    assert!(stderr.contains("boom"), "stderr: {stderr}");
    assert!(stderr.contains("throws.js"), "stderr: {stderr}");
}

#[test]
fn raw_ops_are_not_visible_to_user_code() {
    run("no_private_binding.js").success().stdout("none\n");
}

#[test]
fn console_levels_route_to_stdout_and_stderr() {
    run("console_levels.js")
        .success()
        .stdout("a b 5 { x: 1 }\ninfo\ndebug\n")
        .stderr("warn\nerror\n");
}

#[test]
fn missing_file_exits_1() {
    Command::cargo_bin("velyr")
        .unwrap()
        .args(["run", "/nonexistent/file.js"])
        .assert()
        .code(1);
}

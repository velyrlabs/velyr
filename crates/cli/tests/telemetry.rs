use assert_cmd::Command;

fn velyr() -> Command {
    let mut cmd = Command::cargo_bin("velyr").unwrap();
    cmd.env_remove("VELYR_LOG");
    cmd
}

#[test]
fn json_log_format_keeps_stdout_clean_and_stderr_valid_json() {
    let output = velyr()
        .env("VELYR_LOG", "debug")
        .args(["--log-format", "json", "--version"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!("velyr ", env!("CARGO_PKG_VERSION"), "\n")
    );
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("stderr line is valid JSON");
    }
}

#[test]
fn stderr_is_empty_without_velyr_log() {
    velyr().arg("--version").assert().success().stderr("");
}

#[test]
fn invalid_log_format_is_rejected() {
    velyr()
        .args(["--log-format", "xml", "--version"])
        .assert()
        .failure();
}

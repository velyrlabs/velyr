use std::path::PathBuf;

use assert_cmd::Command;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/permissions")
}

/// Runs `velyr` inside a fixture directory; returns (success, stderr).
fn velyr(cwd: &str, args: &[&str]) -> (bool, String) {
    let output = Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(fixtures().join(cwd))
        .args(args)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn allow_all_warns_on_stderr() {
    let (ok, stderr) = velyr("commented", &["run", "-A", "main.js"]);
    assert!(ok, "{stderr}");
    assert!(stderr.contains("--allow-all"), "{stderr}");
}

#[test]
fn scoped_flag_prints_no_warning() {
    let (ok, stderr) = velyr("commented", &["run", "--allow-read=./data", "main.js"]);
    assert!(ok, "{stderr}");
    assert_eq!(stderr, "");
}

#[test]
fn config_with_comments_and_trailing_comma_parses() {
    let (ok, stderr) = velyr("commented", &["run", "main.js"]);
    assert!(ok, "{stderr}");
    assert_eq!(stderr, "");
}

#[test]
fn unknown_config_key_fails_naming_the_key() {
    let (ok, stderr) = velyr("unknown-key", &["run", "main.js"]);
    assert!(!ok);
    assert!(stderr.contains("bogusKey"), "{stderr}");
}

#[test]
fn invalid_net_port_fails() {
    let (ok, stderr) = velyr(
        "commented",
        &["run", "--allow-net=host:notaport", "main.js"],
    );
    assert!(!ok);
    assert!(stderr.contains("--allow-net"), "{stderr}");
}

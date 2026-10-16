//! `--experimental-shared-code-cache` (VELYR-60): the second worker consumes the code cache the
//! first one published; without the flag nothing is shared and the output is the same.

use std::time::Duration;

use assert_cmd::Command;

fn run(flags: &[&str]) -> std::process::Output {
    let path = format!(
        "{}/../../tests/fixtures/shared-code-cache/main.mjs",
        env!("CARGO_MANIFEST_DIR")
    );
    Command::cargo_bin("velyr")
        .unwrap()
        .env("NO_COLOR", "1")
        .env("VELYR_LOG", "velyr_runtime_core=debug")
        .timeout(Duration::from_secs(30))
        .args(flags)
        .arg("run")
        .arg(&path)
        .output()
        .unwrap()
}

#[test]
fn second_worker_consumes_the_cache_of_the_first() {
    let out = run(&["--experimental-shared-code-cache"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42,42\n");
    assert!(stderr.contains("accepted: 2"), "{stderr}");
    assert!(!stderr.contains("rejected: 1"), "{stderr}");
}

#[test]
fn without_the_flag_nothing_is_shared() {
    let out = run(&[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42,42\n");
    assert!(!stderr.contains("velyr.core.code_cache"), "{stderr}");
}

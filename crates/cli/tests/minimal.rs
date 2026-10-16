//! The `profile-minimal` binary: no web, npm or TypeScript crates, and clear errors for the
//! commands that need them. Run with `--no-default-features --features profile-minimal`.
#![cfg(not(feature = "web"))]

use std::process::Command;

fn velyr() -> Command {
    Command::new(env!("CARGO_BIN_EXE_velyr"))
}

fn run_script(code: &str) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.js");
    std::fs::write(&file, code).unwrap();
    velyr().arg("run").arg(&file).output().unwrap()
}

#[test]
fn an_empty_script_runs() {
    assert!(run_script("").status.success());
}

#[test]
fn fetch_is_not_defined() {
    let output = run_script(
        "if (typeof fetch !== 'undefined') throw new Error('fetch exists'); fetch('x');",
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ReferenceError"), "{stderr}");
}

#[test]
fn commands_of_larger_profiles_say_so() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["serve", "main.js"],
        vec!["add", "left-pad"],
        vec!["remove", "left-pad"],
        vec!["install"],
    ] {
        let output = velyr()
            .args(&args)
            .current_dir(dir.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            stderr.contains("not available in profile minimal"),
            "{args:?}: {stderr}"
        );
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
    }
}

#[test]
fn require_is_not_defined() {
    let output =
        run_script("if (typeof require !== 'undefined') throw new Error('require exists');");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn node_builtins_are_unavailable_with_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.mjs");
    std::fs::write(
        &file,
        "import path from 'node:path'; console.log(path.sep);",
    )
    .unwrap();
    let output = velyr().arg("run").arg(&file).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("node:path") && stderr.contains("not linked"),
        "{stderr}"
    );
}

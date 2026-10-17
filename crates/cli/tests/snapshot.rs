//! Startup snapshot (VELYR-232): `velyr` boots its isolates from the blob `build.rs` baked in, with
//! the same globals, op authority and bootstrap stack names as the cold path (`VELYR_NO_SNAPSHOT=1`).

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use assert_cmd::Command;

const BOOTED: &str = "isolate booted from the startup snapshot";

/// Every own key of `globalThis` and `Velyr` with its descriptor flags and the `typeof` of its
/// value (or of its accessor pair), read without triggering getters.
const DUMP_GLOBALS: &str = r#"
const describe = (target) => (key) => {
  const d = Object.getOwnPropertyDescriptor(target, key);
  const kind = "value" in d ? typeof d.value : `get:${typeof d.get} set:${typeof d.set}`;
  return `${String(key)} ${kind} w=${d.writable} e=${d.enumerable} c=${d.configurable}`;
};
console.log(Reflect.ownKeys(globalThis).map(describe(globalThis)).join("\n"));
console.log(Reflect.ownKeys(Velyr).map(describe(Velyr)).join("\n"));
"#;

fn velyr(snapshot: bool, cwd: &Path, args: &[&str]) -> Output {
    let mut command = Command::cargo_bin("velyr").unwrap();
    command
        .current_dir(cwd)
        .env("VELYR_LOG", "velyr.cli.snapshot=debug")
        .env("NO_COLOR", "1")
        .timeout(Duration::from_secs(30))
        .arg("run")
        .args(args);
    if !snapshot {
        command.env("VELYR_NO_SNAPSHOT", "1");
    }
    command.output().unwrap()
}

fn script(dir: &Path, name: &str, text: &str) -> String {
    std::fs::write(dir.join(name), text).unwrap();
    name.into()
}

fn stdout(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn boots(output: &Output) -> usize {
    String::from_utf8_lossy(&output.stderr)
        .matches(BOOTED)
        .count()
}

fn fixtures(dir: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(dir)
}

#[test]
fn run_boots_from_the_snapshot_unless_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let file = script(dir.path(), "empty.js", "");
    assert_eq!(boots(&velyr(true, dir.path(), &[&file])), 1);
    assert_eq!(boots(&velyr(false, dir.path(), &[&file])), 0);
}

#[test]
fn globals_match_with_and_without_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let file = script(dir.path(), "globals.js", DUMP_GLOBALS);
    let with = stdout(&velyr(true, dir.path(), &[&file]));
    let without = stdout(&velyr(false, dir.path(), &[&file]));
    assert!(
        with.contains("fetch function w=true e=false c=true"),
        "{with}"
    );
    assert!(!with.contains("velyr.snapshot"), "{with}");
    assert!(!with.contains("op_"), "{with}");
    // V8 adds its flag-gated globals (Temporal, WebAssembly, ...) after a context deserializes,
    // so only their position among the non-enumerable keys differs.
    let sorted = |text: &str| {
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        lines.sort();
        lines
    };
    assert_eq!(sorted(&with), sorted(&without));
}

#[test]
fn bootstrap_frames_keep_their_velyr_names() {
    let dir = tempfile::tempdir().unwrap();
    let file = script(
        dir.path(),
        "stack.js",
        "try { new URL('http://['); } catch (e) { console.log(e.stack); }",
    );
    let output = velyr(true, dir.path(), &[&file]);
    assert_eq!(boots(&output), 1);
    let stack = stdout(&output);
    assert!(stack.contains("(velyr:url:"), "{stack}");
}

#[test]
fn ops_called_after_boot_follow_the_isolates_permissions() {
    let dir = fixtures("fs");
    let denied = velyr(true, &dir, &["read-denied.js"]);
    assert_eq!(boots(&denied), 1);
    assert_eq!(stdout(&denied), "VELYR_PERMISSION_DENIED\n");
    let granted = velyr(true, &dir, &["--allow-read=.", "read.js"]);
    assert_eq!(stdout(&granted), "hello fs\n");
}

#[test]
fn workers_boot_from_the_snapshot_and_stay_narrower_than_their_parent() {
    let dir = fixtures("m5-workers");
    let output = velyr(true, &dir, &["--allow-env=HOME", "permissions.js"]);
    assert!(
        boots(&output) > 1,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        stdout(&output),
        "inherit HOME:granted USER:denied\n\
         narrow HOME:granted USER:denied\n\
         wider HOME:granted USER:denied\n"
    );
}

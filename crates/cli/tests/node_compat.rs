//! The node and full profiles wire `velyr-compat-node`: `node:` imports, `require('events')`,
//! `Buffer`, `process` and `setImmediate`, with the facades in a private cache directory.
#![cfg(feature = "node")]

use std::path::Path;

use assert_cmd::Command;

fn run_in(home: &Path, name: &str) -> assert_cmd::assert::Assert {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/node-compat");
    Command::cargo_bin("velyr")
        .unwrap()
        .env("HOME", home)
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("LOCALAPPDATA", home.join("cache"))
        .current_dir(dir)
        .args(["run", name])
        .assert()
}

fn run(name: &str) -> assert_cmd::assert::Assert {
    run_in(tempfile::tempdir().unwrap().path(), name)
}

#[test]
fn esm_imports_node_builtins_with_and_without_the_prefix() {
    run("path.mjs").success().stdout("a/b c/d\n");
}

#[test]
fn cjs_requires_events() {
    run("events.cjs").success().stdout("hello velyr\n");
}

#[test]
fn buffer_is_a_global() {
    run("buffer.js").success().stdout("6869\n");
}

#[test]
fn next_tick_runs_before_promise_callbacks_and_immediates_after() {
    run("tick.js")
        .success()
        .stdout("sync string\ntick\npromise\nimmediate\n");
}

#[test]
fn process_exit_sets_the_exit_code() {
    run("exit.js").code(3).stdout("before\n");
}

#[cfg(unix)]
#[test]
fn facades_live_in_a_private_directory() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    run_in(home.path(), "path.mjs").success();
    let facades = std::fs::read_dir(home.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .flat_map(|dir| walk(&dir))
        .find(|path| path.ends_with("node-facades"))
        .expect("facade directory");
    let mode = std::fs::metadata(&facades).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[cfg(unix)]
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = vec![dir.to_path_buf()];
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.metadata().is_ok_and(|meta| meta.is_dir()) {
            found.extend(walk(&entry.path()));
        }
    }
    found
}

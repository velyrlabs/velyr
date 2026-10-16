//! `velyr run main.ts`: types are stripped by the module host, results cached on disk.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/typescript")
        .join(name)
}

/// Runs with a throwaway home so the transform cache lands in `home` on every platform.
fn run(name: &str, home: &Path) -> assert_cmd::assert::Assert {
    Command::cargo_bin("velyr")
        .unwrap()
        .env("HOME", home)
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("LOCALAPPDATA", home.join("cache"))
        .arg("run")
        .arg(fixture(name))
        .assert()
}

fn cache_files(home: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
    let mut found = Vec::new();
    let mut pending = vec![home.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let meta = entry.metadata().unwrap();
            if entry.file_name() == "node-facades" {
                // Rewritten on every start by design; not the transform cache.
                continue;
            }
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                found.push((entry.path(), meta.modified().unwrap()));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn runs_typescript_with_enum_and_type_only_imports() {
    let home = tempfile::tempdir().unwrap();
    run("main.ts", home.path())
        .success()
        .stdout("square:4:Green\n0 Green\n");
}

#[test]
fn second_run_is_served_from_the_cache() {
    let home = tempfile::tempdir().unwrap();
    run("main.ts", home.path()).success();
    let first = cache_files(home.path());
    assert!(!first.is_empty(), "first run wrote no cache entry");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    run("main.ts", home.path())
        .success()
        .stdout("square:4:Green\n0 Green\n");
    assert_eq!(cache_files(home.path()), first);
}

#[test]
fn syntax_error_reports_location_and_code() {
    let home = tempfile::tempdir().unwrap();
    let assert = run("broken.ts", home.path()).failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("VELYR_INVALID_DATA"), "{stderr}");
    assert!(stderr.contains("broken.ts:2:16"), "{stderr}");
}

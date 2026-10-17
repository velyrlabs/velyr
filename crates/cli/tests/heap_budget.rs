//! `quotas.memoryMb` becomes the heap limit of every isolate: the main one and its workers.

use std::path::Path;
use std::time::Duration;

use assert_cmd::Command;

/// Keeps allocating up to ~600 MB; only a heap limit below that can stop it.
const HOG: &str = r#"
const keep = [];
for (let i = 0; i < 800; i += 1) keep.push(new Array(100000).fill(1));
console.log("survived");
"#;

const SPAWN_HOG: &str = r#"
const worker = new Worker(new URL("./hog.js", import.meta.url), { type: "module" });
worker.addEventListener("error", (event) => {
  console.log("worker failed");
  event.preventDefault();
});
worker.addEventListener("message", () => console.log("worker survived"));
"#;

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    dir
}

fn run(dir: &Path, entry: &str) -> assert_cmd::assert::Assert {
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir)
        .timeout(Duration::from_secs(60))
        .args(["run", entry])
        .assert()
}

const SMALL_BUDGET: &str = r#"{"quotas": {"memoryMb": 128}}"#;

#[test]
fn the_main_isolate_gets_the_budget_heap_limit() {
    let dir = project(&[("velyr.json", SMALL_BUDGET), ("hog.js", HOG)]);
    let output = run(dir.path(), "hog.js").failure().get_output().clone();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("heap quota exceeded"), "{stderr}");
}

#[test]
fn workers_inherit_the_budget_heap_limit() {
    let hog = format!("{HOG}\npostMessage(1);");
    let dir = project(&[
        ("velyr.json", SMALL_BUDGET),
        ("hog.js", &hog),
        ("main.js", SPAWN_HOG),
    ]);
    run(dir.path(), "main.js")
        .success()
        .stdout("worker failed\n");
}

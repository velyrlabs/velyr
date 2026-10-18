//! `velyr test`: pass/fail exit codes, `node:test`, `--filter`, the tap reporter and permissions.
//! Fixtures are written to a temp dir so they are not picked up as tests of this repository.

use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

use assert_cmd::Command;

const PASSING: &str = r#"
import { describe, it, expect, beforeEach, mock } from 'velyr:test';
const order = [];
describe('math', () => {
  beforeEach(() => { order.push('before'); });
  it('adds', () => {
    order.push('adds');
    expect(1 + 1).toBe(2);
    expect({ a: [1, 2] }).toEqual({ a: [1, 2] });
    expect({ a: 1, b: 2 }).toMatchObject({ a: 1 });
    expect(() => { throw new Error('boom'); }).toThrow('boom');
  });
  it('mocks', () => {
    const fn = mock.fn((x) => x * 2);
    expect(fn(2)).toBe(4);
    expect(fn.mock.calls.length).toBe(1);
  });
  it('resolves', async () => {
    await expect(Promise.resolve(3)).resolves.toBe(3);
    await expect(Promise.reject(new Error('no'))).rejects.toThrow('no');
  });
  it('runs hooks first', () => {
    expect(order.slice(0, 2)).toEqual(['before', 'adds']);
  });
});
"#;

const FAILING: &str = r#"
import { it, expect } from 'velyr:test';
it('wrong sum', () => { expect(1 + 1).toBe(3); });
"#;

const NODE_TEST: &str = r#"
import test from 'node:test';
import assert from 'node:assert';
test('node style', () => { assert.strictEqual(2, 2); });
"#;

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in files {
        fs::write(dir.path().join(name), body).unwrap();
    }
    dir
}

fn velyr_test(dir: &Path, args: &[&str]) -> Output {
    velyr_test_jobs(dir, "2", args)
}

fn velyr_test_jobs(dir: &Path, jobs: &str, args: &[&str]) -> Output {
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir)
        .env("HOME", dir)
        .arg("test")
        .args(["--jobs", jobs])
        .args(args)
        .timeout(Duration::from_secs(120))
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_passing_file_succeeds() {
    let dir = project(&[("ok.test.mjs", PASSING)]);
    let out = velyr_test(dir.path(), &[]);
    assert!(out.status.success(), "{}\n{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out).contains("4 passed, 0 failed"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_failing_file_makes_the_exit_code_non_zero() {
    let dir = project(&[("ok.test.mjs", PASSING), ("bad.test.mjs", FAILING)]);
    let out = velyr_test(dir.path(), &[]);
    assert!(!out.status.success());
    let text = stdout(&out);
    assert!(text.contains("4 passed, 1 failed"), "{text}");
    assert!(text.contains("wrong sum"), "{text}");
}

#[test]
fn node_test_and_node_assert_work() {
    let dir = project(&[("n.test.mjs", NODE_TEST)]);
    let out = velyr_test(dir.path(), &[]);
    assert!(out.status.success(), "{}\n{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out).contains("1 passed, 0 failed"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn filter_runs_only_matching_tests() {
    let dir = project(&[("ok.test.mjs", PASSING), ("bad.test.mjs", FAILING)]);
    let out = velyr_test(dir.path(), &["--filter", "math > adds"]);
    assert!(out.status.success(), "{}\n{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out).contains("1 passed, 0 failed"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn tap_reporter_prints_tap() {
    let dir = project(&[("ok.test.mjs", PASSING), ("bad.test.mjs", FAILING)]);
    let out = velyr_test(dir.path(), &["--reporter=tap"]);
    let text = stdout(&out);
    assert!(text.starts_with("TAP version 13"), "{text}");
    assert!(text.contains("ok 1 - "), "{text}");
    assert!(text.contains("not ok "), "{text}");
    assert!(!out.status.success());
}

const READS_FILE: &str = r#"
import { it, expect } from 'velyr:test';
import { readFileSync } from 'node:fs';
it('reads data', () => { expect(readFileSync('data.txt', 'utf8')).toBe('hi'); });
"#;

#[test]
fn a_test_without_permission_fails_and_flags_grant_it() {
    let dir = project(&[("read.test.mjs", READS_FILE), ("data.txt", "hi")]);
    let denied = velyr_test(dir.path(), &[]);
    assert!(!denied.status.success(), "{}", stdout(&denied));
    assert!(
        stdout(&denied).contains("reads data"),
        "{}",
        stdout(&denied)
    );
    let granted = velyr_test(dir.path(), &["--allow-read=."]);
    assert!(
        granted.status.success(),
        "{}\n{}",
        stdout(&granted),
        stderr(&granted)
    );
}

const SPINS: &str = r#"
import { it } from 'velyr:test';
it('spins forever', () => { while (true) {} });
"#;

const NEVER_SETTLES: &str = r#"
import { it, expect } from 'velyr:test';
it('never settles', () => new Promise(() => {}));
it('runs after the timeout', () => { expect(1).toBe(1); });
"#;

#[test]
fn a_test_that_spins_times_out_and_the_other_files_still_run() {
    let dir = project(&[("ok.test.mjs", PASSING), ("spin.test.mjs", SPINS)]);
    let out = velyr_test(dir.path(), &["--timeout", "300"]);
    let text = stdout(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("spins forever"), "{text}");
    assert!(text.contains("timed out"), "{text}");
    assert!(text.contains("4 passed, 1 failed"), "{text}");
}

#[test]
fn an_async_test_that_never_settles_fails_and_the_next_test_runs() {
    let dir = project(&[("never.test.mjs", NEVER_SETTLES)]);
    let out = velyr_test(dir.path(), &["--timeout", "200"]);
    let text = stdout(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("1 passed, 1 failed"), "{text}");
    assert!(text.contains("timed out after 200 ms"), "{text}");
}

fn json_report(out: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(out)).unwrap_or_else(|e| panic!("{e}: {}", stdout(out)))
}

/// File, test name and status of every test, without the durations.
fn verdicts(report: &serde_json::Value) -> Vec<(String, String, String)> {
    let text = |v: &serde_json::Value| v.as_str().unwrap().to_owned();
    let mut found = Vec::new();
    for file in report["files"].as_array().unwrap() {
        for test in file["tests"].as_array().unwrap() {
            found.push((
                text(&file["file"]),
                text(&test["name"]),
                text(&test["status"]),
            ));
        }
    }
    found
}

#[test]
fn one_job_and_two_jobs_give_the_same_result() {
    let dir = project(&[("ok.test.mjs", PASSING), ("bad.test.mjs", FAILING)]);
    let one = json_report(&velyr_test_jobs(dir.path(), "1", &["--reporter=json"]));
    let two = json_report(&velyr_test_jobs(dir.path(), "2", &["--reporter=json"]));
    assert_eq!(verdicts(&one), verdicts(&two));
    assert_eq!(one["summary"]["pass"], 4);
    assert_eq!(one["summary"]["fail"], 1);
    assert_eq!(one["summary"]["pass"], two["summary"]["pass"]);
}

#[test]
fn the_json_reporter_prints_one_valid_document() {
    let dir = project(&[("ok.test.mjs", PASSING), ("bad.test.mjs", FAILING)]);
    let out = velyr_test(dir.path(), &["--reporter=json"]);
    assert!(!out.status.success());
    let report = json_report(&out);
    assert_eq!(report["summary"]["files"], 2);
    assert_eq!(report["summary"]["failedFiles"], 1);
}

#[test]
fn the_junit_reporter_prints_xml_with_a_failure() {
    let dir = project(&[("ok.test.mjs", PASSING), ("bad.test.mjs", FAILING)]);
    let out = velyr_test(dir.path(), &["--reporter=junit"]);
    let text = stdout(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.starts_with("<?xml version=\"1.0\""), "{text}");
    assert_eq!(text.matches("<testcase ").count(), 5, "{text}");
    assert_eq!(text.matches("<failure ").count(), 1, "{text}");
    assert!(text.trim_end().ends_with("</testsuites>"), "{text}");
}

#[test]
fn a_bare_require_of_test_is_not_the_runner_but_node_test_is() {
    let dir = project(&[(
        "bare.cjs",
        "let bare; try { bare = typeof require('test'); } catch (e) { bare = e.code; }\n\
         console.log(bare + ',' + typeof require('node:test'));\n",
    )]);
    let out = Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["run", "--allow-read=.", "bare.cjs"])
        .timeout(Duration::from_secs(60))
        .output()
        .unwrap();
    assert_eq!(
        stdout(&out).trim(),
        "MODULE_NOT_FOUND,function",
        "{}",
        stderr(&out)
    );
}

struct Watcher {
    child: std::process::Child,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Watcher {
    fn start(dir: &Path) -> Self {
        use std::io::{BufRead, BufReader};
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_velyr"))
            .current_dir(dir)
            .env("HOME", dir)
            .args(["test", "--watch", "--jobs", "1"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { child, lines }
    }

    fn wait_for(&self, want: &str) {
        assert!(
            self.saw(want, Duration::from_secs(60)),
            "no line with `{want}` within 60 s"
        );
    }

    fn saw(&self, want: &str, within: Duration) -> bool {
        let end = std::time::Instant::now() + within;
        while let Ok(line) = self
            .lines
            .recv_timeout(end.saturating_duration_since(std::time::Instant::now()))
        {
            if line.contains(want) {
                return true;
            }
        }
        false
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn watch_reruns_the_tests_when_a_test_file_is_edited() {
    let one = "import { it } from 'velyr:test';\nit('first', () => {});\n";
    let two = "import { it } from 'velyr:test';\nit('first', () => {});\nit('second', () => {});\n";
    let dir = project(&[("w.test.mjs", one)]);
    let root = dunce::canonicalize(dir.path()).unwrap();
    let watcher = Watcher::start(&root);
    watcher.wait_for("1 passed, 0 failed");
    // The test file is watched once the supervisor polls the module report, so an edit made
    // before that on a loaded runner goes unseen; keep editing until the rerun shows up.
    let rerun = (0..12).any(|_| {
        fs::write(root.join("w.test.mjs"), two).unwrap();
        watcher.saw("2 passed, 0 failed", Duration::from_secs(5))
    });
    assert!(rerun, "no rerun with `2 passed, 0 failed` within 60 s");
}

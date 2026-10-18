//! `velyr bench` (pretty and json reports) and `velyr check` (external tsc, ADR-012).

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

use assert_cmd::Command;

const BENCH: &str = r#"
Velyr.bench('sum', () => { let s = 0; for (let i = 0; i < 10000; i++) s += i; return s; });
Velyr.bench('async work', async () => { await Promise.resolve(1); });
"#;

const FAKE_TSC: &str = "#!/usr/bin/env node\nconsole.log('tsc-args:' + process.argv.slice(2).join(' '));\nprocess.exit(Number(process.env.FAKE_TSC_EXIT || 0));\n";

fn velyr(dir: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir)
        .env("HOME", dir)
        .args(args)
        .timeout(Duration::from_secs(120))
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn bench_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("b.bench.mjs"), BENCH).unwrap();
    dir
}

#[cfg(unix)]
fn install_fake_tsc(dir: &Path) {
    let bin = dir.join("node_modules/.bin");
    fs::create_dir_all(&bin).unwrap();
    let tsc = bin.join("tsc");
    fs::write(&tsc, FAKE_TSC).unwrap();
    fs::set_permissions(&tsc, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn bench_prints_a_pretty_table() {
    let dir = bench_dir();
    let out = velyr(
        dir.path(),
        &["bench", "b.bench.mjs", "--warmup", "2", "--samples", "10"],
    );
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", text(&out.stderr));
    for needle in ["sum", "async work", "mean", "p50", "p99", "ops/s"] {
        assert!(stdout.contains(needle), "missing {needle}: {stdout}");
    }
}

#[test]
fn bench_json_has_the_statistics() {
    let dir = bench_dir();
    let out = velyr(
        dir.path(),
        &[
            "bench",
            "b.bench.mjs",
            "--json",
            "--warmup",
            "2",
            "--samples",
            "10",
        ],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let benches = report["benches"].as_array().unwrap();
    assert_eq!(benches.len(), 2);
    let first = &benches[0];
    assert_eq!(first["name"], "sum");
    assert_eq!(first["samples"], 10);
    // Under a coarsened timer (macOS CI) even a batched sample can read 0 ns: the mean is then 0 and
    // opsPerSec is Infinity, which JSON serializes as null.
    for key in ["meanNs", "p99Ns"] {
        assert!(first[key].as_f64().unwrap() >= 0.0, "{key}: {first}");
    }
    let ops = &first["opsPerSec"];
    assert!(
        ops.is_null() || ops.as_f64().unwrap() > 0.0,
        "opsPerSec: {first}"
    );
    // A sample faster than the coarsened timer resolution reads as 0, so the median can be 0.
    assert!(first["p50Ns"].as_f64().unwrap() >= 0.0, "p50Ns: {first}");
    assert!(first["p99Ns"].as_f64() >= first["p50Ns"].as_f64());
}

#[test]
fn bench_filter_keeps_matching_names() {
    let dir = bench_dir();
    let out = velyr(
        dir.path(),
        &[
            "bench",
            "b.bench.mjs",
            "--json",
            "--samples",
            "5",
            "--filter",
            "async",
        ],
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["benches"].as_array().unwrap().len(), 1);
}

#[test]
fn check_without_typescript_fails_with_a_clear_message() {
    let dir = tempfile::tempdir().unwrap();
    let out = velyr(dir.path(), &["check"]);
    assert!(!out.status.success());
    let stderr = text(&out.stderr);
    assert!(stderr.contains("TypeScript is not installed"), "{stderr}");
    assert!(stderr.contains("velyr add -D typescript"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn check_runs_tsc_with_no_emit() {
    let dir = tempfile::tempdir().unwrap();
    install_fake_tsc(dir.path());
    let out = velyr(dir.path(), &["check"]);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", text(&out.stderr));
    assert!(stdout.contains("tsc-args:--noEmit"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn check_passes_files_through() {
    let dir = tempfile::tempdir().unwrap();
    install_fake_tsc(dir.path());
    let out = velyr(dir.path(), &["check", "--files", "a.ts", "src/b.ts"]);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("tsc-args:--noEmit a.ts src/b.ts"),
        "{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn check_exits_non_zero_when_tsc_reports_errors() {
    let dir = tempfile::tempdir().unwrap();
    install_fake_tsc(dir.path());
    let out = Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("FAKE_TSC_EXIT", "2")
        .arg("check")
        .output()
        .unwrap();
    assert!(!out.status.success());
}

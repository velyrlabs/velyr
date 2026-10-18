// velyr-bench measures children with getrusage and refuses to run on a non-unix host.
#![cfg(unix)]

use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn bench() -> Command {
    Command::cargo_bin("velyr-bench").unwrap()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn schema() -> serde_json::Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benches/schema/result.schema.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn run_suite(suite: &Path, out: &Path) -> serde_json::Value {
    bench()
        .arg("run")
        .arg(suite)
        .arg("--out-dir")
        .arg(out)
        .assert()
        .success();
    let files: Vec<_> = std::fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("trivial-") && name.ends_with(".json"),
        "{name}"
    );
    serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap()
}

#[test]
fn run_writes_result_that_validates_against_schema() {
    let out = tempdir("run-valid");
    let result = run_suite(&fixture("true.toml"), &out);

    let validator = jsonschema::validator_for(&schema()).unwrap();
    let errors: Vec<_> = validator
        .iter_errors(&result)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(result["benchmark"], "trivial");
    for metric in ["wall_ms", "rss_mb", "cpu_seconds"] {
        assert!(result["metrics"][metric].is_number(), "{metric}");
    }
}

#[test]
fn run_fails_when_command_cannot_start() {
    let out = tempdir("run-missing");
    let suite = out.join("bad.toml");
    std::fs::write(
        &suite,
        "name='bad'\ncommand=['/nonexistent/bin']\niterations=1\nwarmup=0\nmetrics=['wall_ms']\n",
    )
    .unwrap();
    bench()
        .arg("run")
        .arg(&suite)
        .arg("--out-dir")
        .arg(&out)
        .assert()
        .failure();
}

#[test]
fn compare_exits_1_when_throughput_is_10_percent_slower() {
    bench()
        .arg("compare")
        .arg(fixture("baseline.json"))
        .arg(fixture("slower.json"))
        .assert()
        .code(1);
}

#[test]
fn compare_exits_1_when_p99_regresses_beyond_budget() {
    bench()
        .arg("compare")
        .arg(fixture("baseline.json"))
        .arg(fixture("worse_p99.json"))
        .assert()
        .code(1);
}

#[test]
fn compare_exits_0_when_faster_or_equal() {
    bench()
        .arg("compare")
        .arg(fixture("baseline.json"))
        .arg(fixture("faster.json"))
        .assert()
        .success();
    bench()
        .arg("compare")
        .arg(fixture("baseline.json"))
        .arg(fixture("baseline.json"))
        .assert()
        .success();
}

#[test]
fn compare_exits_1_when_rps_is_20_percent_lower() {
    let out = bench()
        .arg("compare")
        .arg(fixture("baseline.json"))
        .arg(fixture("rps_20_lower.json"))
        .assert()
        .code(1);
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    assert!(
        stdout.contains("requests_per_sec") && stdout.contains("FAIL"),
        "{stdout}"
    );
}

/// A suite whose "driver" just copies a canned JSON, so no server or load runs.
fn server_suite(dir: &Path, metrics: &str, driver_json: &str) -> PathBuf {
    let suite = dir.join("server.toml");
    std::fs::write(
        &suite,
        format!(
            "name='http-json'\nmode='server'\ncommand=['cp','{}','{}']\ndriver_json='{}'\n\
             iterations=3\nwarmup=1\nmetrics={metrics}\n",
            fixture("driver-output.json").display(),
            dir.join("driver.json").display(),
            driver_json,
        ),
    )
    .unwrap();
    suite
}

#[test]
fn server_mode_exposes_rps_and_p99_from_the_driver_json() {
    let out = tempdir("server");
    let driver = out.join("driver.json").display().to_string();
    let suite = server_suite(
        &out,
        "['requests_per_sec','p99_ms','rss_mb','wall_ms']",
        &driver,
    );
    bench()
        .arg("run")
        .arg(&suite)
        .arg("--out-dir")
        .arg(out.join("res"))
        .assert()
        .success();
    let file = std::fs::read_dir(out.join("res"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    assert!(
        jsonschema::validator_for(&schema())
            .unwrap()
            .is_valid(&result),
        "{result}"
    );
    let m = &result["metrics"];
    assert_eq!(m["requests_per_sec"].as_f64().unwrap(), 40000.5);
    assert_eq!(m["p99_ms"].as_f64().unwrap(), 2.5);
    assert_eq!(m["rss_mb"].as_f64().unwrap(), 100.0);
    assert!(m["wall_ms"].is_number());
}

#[test]
fn server_mode_fails_when_the_driver_wrote_nothing_usable() {
    let out = tempdir("server-missing");
    let suite = server_suite(&out, "['requests_per_sec']", "/nonexistent/driver.json");
    let run = bench()
        .arg("run")
        .arg(&suite)
        .arg("--out-dir")
        .arg(out.join("res"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&run.get_output().stderr).into_owned();
    assert!(stderr.contains("driver.json"), "{stderr}");
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("velyr-bench-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn footprint_metrics_cover_binary_size_p95_and_isolates() {
    let out = tempdir("footprint");
    let suite = out.join("footprint.toml");
    std::fs::write(
        &suite,
        "name='fp'\ncommand=['velyr','--version']\niterations=5\nwarmup=1\n\
         metrics=['wall_ms','wall_ms_p95','rss_mb','binary_bytes','binary_stripped_bytes','isolates']\n",
    )
    .unwrap();
    let bin = assert_cmd::cargo::cargo_bin("velyr-bench");
    bench()
        .arg("run")
        .arg(&suite)
        .arg("--bin")
        .arg(&bin)
        .arg("--out-dir")
        .arg(out.join("res"))
        .assert()
        .success();
    let file = std::fs::read_dir(out.join("res"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema()).unwrap();
    assert!(validator.is_valid(&result), "{result}");
    let m = &result["metrics"];
    let size = std::fs::metadata(&bin).unwrap().len() as f64;
    assert_eq!(m["binary_bytes"].as_f64().unwrap(), size);
    let stripped = m["binary_stripped_bytes"].as_f64().unwrap();
    assert!(stripped > 0.0 && stripped <= size, "{m}");
    assert!(m["wall_ms_p95"].as_f64().unwrap() >= m["wall_ms"].as_f64().unwrap());
    assert_eq!(m["isolates"].as_f64().unwrap(), 1.0);
}

fn startup_suite(dir: &Path) -> PathBuf {
    let suite = dir.join("startup.toml");
    std::fs::write(
        &suite,
        "name='startup'\ncommand=['velyr','--version']\niterations=3\nwarmup=1\n\
         metrics=['wall_ms','rss_mb','instructions','cycles']\n",
    )
    .unwrap();
    suite
}

fn only_json(dir: &Path) -> serde_json::Value {
    let file = std::fs::read_dir(dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

/// CI VMs often expose no hardware counters; the counter tests skip there instead of failing.
fn hardware_counters_available() -> bool {
    let probe = if cfg!(target_os = "macos") {
        std::process::Command::new("/usr/bin/time")
            .args(["-l", "true"])
            .output()
    } else {
        std::process::Command::new("perf")
            .args(["stat", "-x,", "-e", "instructions", "--", "true"])
            .output()
    };
    let Ok(out) = probe else { return false };
    out.status.success()
        && String::from_utf8_lossy(&out.stderr).lines().any(|line| {
            line.contains("instructions")
                && line
                    .split([',', ' '])
                    .find(|t| !t.is_empty())
                    .and_then(|t| t.parse::<u64>().ok())
                    .is_some_and(|n| n > 0)
        })
}

#[test]
fn startup_run_records_commit_load_average_and_instructions() {
    if !hardware_counters_available() {
        eprintln!("skipped: no hardware performance counters on this host");
        return;
    }
    let out = tempdir("startup");
    let bin = assert_cmd::cargo::cargo_bin("velyr-bench");
    bench()
        .arg("run")
        .arg(startup_suite(&out))
        .arg("--bin")
        .arg(&bin)
        .arg("--out-dir")
        .arg(out.join("res"))
        .assert()
        .success();
    let result = only_json(&out.join("res"));
    assert!(
        jsonschema::validator_for(&schema())
            .unwrap()
            .is_valid(&result),
        "{result}"
    );
    let commit = result["commit"].as_str().unwrap();
    assert!(
        commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "{commit}"
    );
    let load = result["load_average"].as_array().unwrap();
    assert!(load.len() == 3 && load.iter().all(|v| v.as_f64().is_some_and(|n| n >= 0.0)));
    assert!(result["metrics"]["instructions"].as_f64().unwrap() > 0.0);
    assert!(result["metrics"]["cycles"].as_f64().unwrap() > 0.0);
}

#[test]
fn compare_exits_1_when_instructions_regress_5_percent() {
    let out = bench()
        .arg("compare")
        .arg(fixture("instructions_base.json"))
        .arg(fixture("instructions_worse.json"))
        .assert()
        .code(1);
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    assert!(
        stdout.contains("instructions") && stdout.contains("FAIL"),
        "{stdout}"
    );
}

#[test]
fn ab_runs_both_binaries_in_one_session_and_records_the_base_commit() {
    if !hardware_counters_available() {
        eprintln!("skipped: no hardware performance counters on this host");
        return;
    }
    let out = tempdir("ab");
    let bin = assert_cmd::cargo::cargo_bin("velyr-bench");
    let run = bench()
        .arg("ab")
        .arg(startup_suite(&out))
        .arg("--base-bin")
        .arg(&bin)
        .arg("--base-commit")
        .arg("0123456789abcdef0123456789abcdef01234567")
        .arg("--bin")
        .arg(&bin)
        .arg("--out-dir")
        .arg(out.join("res"))
        .assert();
    // Same binary on both sides: only exit 0 or 1 (noise) are legitimate, never 2 (error).
    assert!(matches!(run.get_output().status.code(), Some(0 | 1)));
    let stdout = String::from_utf8_lossy(&run.get_output().stdout).into_owned();
    assert!(
        stdout.contains("instructions") && !stdout.contains("wall_ms:"),
        "{stdout}"
    );
    assert_eq!(
        only_json(&out.join("res/base"))["commit"],
        "0123456789abcdef0123456789abcdef01234567"
    );
    let new = only_json(&out.join("res/new"));
    assert!(new["metrics"]["instructions"].is_number());
    if cfg!(target_os = "macos") {
        let base_rss = only_json(&out.join("res/base"))["metrics"]["rss_mb"]
            .as_f64()
            .unwrap();
        let new_rss = new["metrics"]["rss_mb"].as_f64().unwrap();
        assert!((base_rss - new_rss).abs() < 1.0, "{base_rss} vs {new_rss}");
    }
}

/// The first invocation (a warmup) allocates ~200 MB; every measured one is a tiny `:`.
/// rss_mb must come from the measured child, not from the harness' children high-water mark.
/// macOS only: Linux has no per-child RSS yet (see the `shortcut:` in main.rs).
#[cfg(target_os = "macos")]
#[test]
fn rss_mb_ignores_a_large_process_that_ran_before_the_measurement() {
    let out = tempdir("rss-isolated");
    let marker = out.join("seen");
    let suite = out.join("rss.toml");
    std::fs::write(
        &suite,
        format!(
            "name='trivial'\niterations=3\nwarmup=1\nmetrics=['wall_ms','rss_mb']\n\
             command=['sh','-c','if [ -e {0} ]; then :; else touch {0}; \
             perl -e \"print length(q(x) x 200000000)\"; fi']\n",
            marker.display()
        ),
    )
    .unwrap();
    let result = run_suite(&suite, &out.join("res"));
    let rss = result["metrics"]["rss_mb"].as_f64().unwrap();
    assert!(rss < 50.0, "rss_mb leaked the warmup process: {rss}");
}

#[test]
fn committed_baselines_validate_against_schema() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benches/baselines");
    let validator = jsonschema::validator_for(&schema()).unwrap();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        // Driver side files (comparison, summary, multicore) are not results in the comparable schema.
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let side_file = ["-comparison-", "m4-summary-", "m5-multicore-"]
            .iter()
            .any(|marker| name.contains(marker));
        if path.extension().is_some_and(|e| e == "json") && !side_file {
            let json = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let errors: Vec<_> = validator
                .iter_errors(&json)
                .map(|e| e.to_string())
                .collect();
            assert!(errors.is_empty(), "{}: {errors:?}", path.display());
        }
    }
}

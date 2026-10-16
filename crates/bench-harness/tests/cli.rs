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

//! `velyr-bench`: runs benchmark suites and compares results against baselines.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use nix::sys::resource::{UsageWho, getrusage};
use serde::{Deserialize, Serialize};
use sysinfo::System;

/// Merge budgets from docs/PERFORMANCE-BUDGETS.md: (metric, higher is better, max regression %).
const BUDGETS: &[(&str, bool, f64)] = &[
    ("requests_per_sec", true, 3.0),
    ("p99_ms", false, 5.0),
    ("rss_mb", false, 3.0),
    ("wall_ms", false, 5.0),
];

#[derive(Parser)]
#[command(version, about = "Velyr benchmark harness")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a suite and write `<out-dir>/<suite>-<unix-ts>.json`.
    Run {
        /// Suite file (benches/suites/*.toml).
        suite: PathBuf,
        /// Binary substituted for a leading `velyr` in the suite command.
        #[arg(long, env = "VELYR_BIN", default_value = "target/release/velyr")]
        bin: PathBuf,
        /// Build profile recorded in the result.
        #[arg(long, default_value = "release")]
        profile: String,
        #[arg(long, default_value = "benches/results")]
        out_dir: PathBuf,
    },
    /// Compare a result with a baseline using the merge budgets; exit 1 on regression.
    Compare { baseline: PathBuf, result: PathBuf },
}

#[derive(Deserialize)]
struct Suite {
    name: String,
    command: Vec<String>,
    iterations: u32,
    warmup: u32,
    metrics: Vec<String>,
    #[serde(default)]
    mode: Mode,
    /// Server mode: result JSON the driver command writes (`{platform}` = driver spelling,
    /// e.g. `darwin-aarch64`); its `metrics` win over the harness' own for the same name.
    driver_json: Option<String>,
}

#[derive(Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Mode {
    /// A command that exits; the harness measures wall/RSS/CPU.
    #[default]
    Command,
    /// A load driver (benches/http/run.py) that reports throughput and latency itself.
    Server,
}

#[derive(Serialize, Deserialize)]
struct BenchResult {
    benchmark: String,
    commit: String,
    profile: String,
    platform: String,
    cpu: String,
    memory: String,
    rustc: String,
    os: String,
    timestamp: String,
    metrics: BTreeMap<String, f64>,
}

fn main() -> ExitCode {
    let outcome = match Cli::parse().command {
        Cmd::Run {
            suite,
            bin,
            profile,
            out_dir,
        } => run(&suite, &bin, profile, &out_dir).map(|()| true),
        Cmd::Compare { baseline, result } => compare(&baseline, &result),
    };
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn read_toml<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_result(path: &Path) -> Result<BenchResult, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn run(suite_path: &Path, bin: &Path, profile: String, out_dir: &Path) -> Result<(), String> {
    let suite: Suite = read_toml(suite_path)?;
    if suite.iterations == 0 || suite.command.is_empty() {
        return Err("suite needs a command and iterations > 0".into());
    }
    let mut cmd = suite.command.clone();
    if cmd[0] == "velyr" {
        cmd[0] = bin.display().to_string();
    }

    for _ in 0..suite.warmup {
        spawn_once(&cmd)?;
    }
    let cpu_before = children_cpu_seconds()?;
    let mut walls = Vec::new();
    let mut driven: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for _ in 0..suite.iterations {
        let start = Instant::now();
        spawn_once(&cmd)?;
        walls.push(start.elapsed().as_secs_f64() * 1000.0);
        if suite.mode == Mode::Server {
            for (name, value) in read_driver_metrics(&suite)? {
                driven.entry(name).or_default().push(value);
            }
        }
    }
    walls.sort_by(f64::total_cmp);
    let cpu_total = children_cpu_seconds()? - cpu_before;
    let usage = getrusage(UsageWho::RUSAGE_CHILDREN).map_err(|e| e.to_string())?;

    let mut metrics = BTreeMap::new();
    for metric in &suite.metrics {
        if let Some(values) = driven.get_mut(metric) {
            values.sort_by(f64::total_cmp);
            metrics.insert(metric.clone(), values[values.len() / 2]);
            continue;
        }
        let value = match metric.as_str() {
            "wall_ms" => walls[walls.len() / 2],
            "wall_ms_p95" => percentile(&walls, 0.95),
            "binary_bytes" => file_size(&cmd[0])? as f64,
            "binary_stripped_bytes" => stripped_size(&cmd[0])? as f64,
            // `velyr run` hosts a single isolate; recorded so baselines state what RSS covers.
            "isolates" => 1.0,
            "cpu_seconds" => cpu_total / f64::from(suite.iterations),
            "rss_mb" => max_rss_mb(usage.max_rss()),
            other => return Err(format!("unknown metric `{other}`")),
        };
        metrics.insert(metric.clone(), value);
    }

    let mut sys = System::new();
    sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
    sys.refresh_memory();
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let result = BenchResult {
        benchmark: suite.name.clone(),
        commit: git_commit(),
        profile,
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        cpu: sys
            .cpus()
            .first()
            .map(|c| c.brand().to_string())
            .unwrap_or_default(),
        memory: format!("{} MiB", sys.total_memory() / (1024 * 1024)),
        rustc: command_stdout("rustc", &["--version"]).unwrap_or_else(|| "unknown".into()),
        os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.into()),
        timestamp: rfc3339_utc(secs),
        metrics,
    };
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let path = out_dir.join(format!("{}-{secs}.json", suite.name));
    let json = serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("{}", path.display());
    Ok(())
}

/// The `metrics` of the driver's result file, read after each driver run.
fn read_driver_metrics(suite: &Suite) -> Result<BTreeMap<String, f64>, String> {
    let template = suite
        .driver_json
        .as_deref()
        .ok_or("server mode needs `driver_json`")?;
    // Same spelling as benches/http/run.py: lowercase system name plus machine.
    let os = if std::env::consts::OS == "macos" {
        "darwin"
    } else {
        std::env::consts::OS
    };
    let path = template.replace("{platform}", &format!("{os}-{}", std::env::consts::ARCH));
    Ok(read_result(Path::new(&path))?.metrics)
}

/// Nearest-rank percentile of an ascending-sorted, non-empty slice.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn file_size(path: &str) -> Result<u64, String> {
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("{path}: {e}"))
}

/// Size after `strip` on a temporary copy; the original binary is never touched.
fn stripped_size(path: &str) -> Result<u64, String> {
    let tmp = std::env::temp_dir().join(format!("velyr-bench-strip-{}", std::process::id()));
    let status = Command::new("strip")
        .arg("-o")
        .arg(&tmp)
        .arg(path)
        .status()
        .map_err(|e| format!("binary_stripped_bytes needs `strip` on PATH: {e}"))?;
    let size = if status.success() {
        file_size(&tmp.display().to_string())
    } else {
        Err(format!("`strip` failed on {path}: {status}"))
    };
    let _ = std::fs::remove_file(&tmp);
    size
}

fn spawn_once(cmd: &[String]) -> Result<(), String> {
    let status = Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("{}: {e}", cmd[0]))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("`{}` exited with {status}", cmd[0]))
}

fn children_cpu_seconds() -> Result<f64, String> {
    let usage = getrusage(UsageWho::RUSAGE_CHILDREN).map_err(|e| e.to_string())?;
    let secs = |t: nix::sys::time::TimeVal| t.tv_sec() as f64 + t.tv_usec() as f64 / 1e6;
    Ok(secs(usage.user_time()) + secs(usage.system_time()))
}

/// ru_maxrss is bytes on macOS and KiB elsewhere; it is a high-water mark over all children.
fn max_rss_mb(raw: i64) -> f64 {
    let bytes = if cfg!(target_os = "macos") {
        raw as f64
    } else {
        raw as f64 * 1024.0
    };
    bytes / (1024.0 * 1024.0)
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_commit() -> String {
    command_stdout("git", &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into())
}

/// Civil-from-days conversion (Howard Hinnant) so no date crate is needed.
fn rfc3339_utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Returns Ok(true) when no budget is exceeded.
fn compare(baseline: &Path, result: &Path) -> Result<bool, String> {
    let (base, new) = (read_result(baseline)?, read_result(result)?);
    let mut ok = true;
    for &(metric, higher_is_better, limit) in BUDGETS {
        let (Some(&b), Some(&n)) = (base.metrics.get(metric), new.metrics.get(metric)) else {
            continue;
        };
        if b <= 0.0 {
            continue;
        }
        let regression = if higher_is_better {
            (b - n) / b
        } else {
            (n - b) / b
        } * 100.0;
        let breached = regression > limit;
        ok &= !breached;
        println!(
            "{metric}: {b} -> {n} ({regression:+.2}% regression, budget {limit}%) {}",
            if breached { "FAIL" } else { "ok" }
        );
    }
    Ok(ok)
}

//! Runs test files in child processes, `jobs` at a time, and kills the ones that run too long.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::report::{FileResult, Status, TestResult, parse_output, running_after};

/// How long the runner may overrun `Limits::test_ms` before the parent kills the file: the
/// runner times out async tests itself, so this only fires for a test that blocks its isolate.
const KILL_GRACE: Duration = Duration::from_millis(500);
/// How often the parent checks its deadlines while a child is quiet.
const POLL: Duration = Duration::from_millis(50);
/// How long to keep collecting output after a child was killed.
const DRAIN: Duration = Duration::from_millis(300);

/// Time limits in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// One test; the one running when it passes is failed and its file is killed.
    pub test_ms: u64,
    /// One whole file.
    pub file_ms: u64,
}

/// Most isolates that run at once by default: each one costs memory.
const MAX_DEFAULT_JOBS: usize = 4;

/// Half the CPUs, at least 1 and at most 4.
pub fn default_jobs() -> usize {
    let cpus = std::thread::available_parallelism().map_or(1, usize::from);
    (cpus / 2).clamp(1, MAX_DEFAULT_JOBS)
}

/// Runs the command `command(file)` builds for every file, `jobs` at a time (at least 1), calls
/// `on_done` with each result as soon as its file ends, and returns the results in the order of
/// `files`.
pub fn run_files(
    files: &[PathBuf],
    jobs: usize,
    limits: Limits,
    command: impl Fn(&Path) -> Command + Sync,
    on_done: impl Fn(&FileResult) + Sync,
) -> Vec<FileResult> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<FileResult>>> = Mutex::new(vec![None; files.len()]);
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, files.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(index) else { break };
                    let result = run_one(file, command(file), limits);
                    on_done(&result);
                    results.lock().expect("results lock")[index] = Some(result);
                }
            });
        }
    });
    let results = results.into_inner().expect("results lock");
    results.into_iter().flatten().collect()
}

/// What made the parent kill a child.
enum Overrun {
    /// The named test went past `Limits::test_ms`.
    Test(Vec<String>),
    File,
}

/// A line from one of the child's pipes.
enum Line {
    Out(String),
    Err(String),
}

fn forward(
    pipe: impl std::io::Read + Send + 'static,
    wrap: fn(String) -> Line,
    tx: mpsc::Sender<Line>,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
            if tx
                .send(wrap(String::from_utf8_lossy(&line).into_owned()))
                .is_err()
            {
                break;
            }
        }
    });
}

fn run_one(file: &Path, mut command: Command, limits: Limits) -> FileResult {
    let name = file.to_string_lossy().into_owned();
    let spawned = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            return FileResult {
                file: name,
                error: Some(format!("cannot start the test process: {err}")),
                ..FileResult::default()
            };
        }
    };
    let (tx, rx) = mpsc::channel();
    if let Some(pipe) = child.stdout.take() {
        forward(pipe, Line::Out, tx.clone());
    }
    if let Some(pipe) = child.stderr.take() {
        forward(pipe, Line::Err, tx.clone());
    }
    drop(tx);
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let mut running: Option<Vec<String>> = None;
    let started = Instant::now();
    let mut running_since = started;
    let mut overrun = None;
    loop {
        match rx.recv_timeout(POLL) {
            Ok(Line::Out(line)) => {
                let before = running.clone();
                running = running_after(&line, running);
                if running != before {
                    running_since = Instant::now();
                }
                stdout.push_str(&line);
                stdout.push('\n');
            }
            Ok(Line::Err(line)) => {
                stderr.push_str(&line);
                stderr.push('\n');
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let test_limit = Duration::from_millis(limits.test_ms) + KILL_GRACE;
        if started.elapsed() > Duration::from_millis(limits.file_ms) {
            overrun = Some(Overrun::File);
        } else if running.is_some() && running_since.elapsed() > test_limit {
            overrun = running.take().map(Overrun::Test);
        }
        if overrun.is_some() {
            kill(&mut child);
            let end = Instant::now() + DRAIN;
            while let Ok(line) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                match line {
                    Line::Out(line) => stdout.push_str(&format!("{line}\n")),
                    Line::Err(line) => stderr.push_str(&format!("{line}\n")),
                }
            }
            break;
        }
    }
    let success = child.wait().is_ok_and(|status| status.success());
    let mut result = parse_output(&name, &stdout, &stderr, success);
    match overrun {
        Some(Overrun::Test(path)) => {
            result.error = None;
            result.tests.push(TestResult {
                path,
                status: Status::Fail,
                ms: limits.test_ms,
                error: Some(format!("timed out after {} ms", limits.test_ms)),
            });
        }
        Some(Overrun::File) => {
            result.error = Some(format!("the file timed out after {} ms", limits.file_ms));
        }
        None => {}
    }
    result
}

fn kill(child: &mut Child) {
    // Already gone is fine: the wait that follows reaps it either way.
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_jobs_is_between_1_and_4() {
        assert!((1..=MAX_DEFAULT_JOBS).contains(&default_jobs()));
    }

    const NO_LIMITS: Limits = Limits {
        test_ms: 60_000,
        file_ms: 60_000,
    };

    #[test]
    fn files_are_reported_as_soon_as_they_finish() {
        let files: Vec<PathBuf> = ["slow", "fast"].map(PathBuf::from).to_vec();
        let done = Mutex::new(Vec::new());
        run_files(
            &files,
            2,
            NO_LIMITS,
            |file| {
                let delay = if file.to_string_lossy() == "slow" {
                    "0.4"
                } else {
                    "0"
                };
                let mut command = Command::new("sh");
                command.args(["-c", &format!("sleep {delay}")]);
                command
            },
            |result| done.lock().unwrap().push(result.file.clone()),
        );
        assert_eq!(done.into_inner().unwrap(), ["fast", "slow"]);
    }

    fn silent_child(script: &'static str) -> impl Fn(&Path) -> Command + Sync {
        move |_| {
            let mut command = Command::new("sh");
            command.args(["-c", script]);
            command
        }
    }

    #[test]
    fn a_test_that_runs_past_its_limit_fails_and_the_child_is_killed() {
        let limits = Limits {
            test_ms: 200,
            file_ms: 60_000,
        };
        let child = silent_child(
            "printf '\\001velyr-test {\"type\":\"start\",\"path\":[\"s\",\"spins\"]}\\n'; exec sleep 30",
        );
        let started = std::time::Instant::now();
        let results = run_files(&[PathBuf::from("a")], 1, limits, child, |_| {});
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        let test = &results[0].tests[0];
        assert_eq!(test.name(), "s > spins");
        assert_eq!(test.status, crate::report::Status::Fail);
        assert!(
            test.error
                .as_deref()
                .unwrap()
                .contains("timed out after 200 ms")
        );
        assert!(results[0].failed());
    }

    #[test]
    fn a_file_that_runs_past_its_ceiling_fails_with_a_timeout_error() {
        let limits = Limits {
            test_ms: 60_000,
            file_ms: 200,
        };
        let results = run_files(
            &[PathBuf::from("a")],
            1,
            limits,
            silent_child("exec sleep 30"),
            |_| {},
        );
        assert!(
            results[0]
                .error
                .as_deref()
                .unwrap()
                .contains("timed out after 200 ms")
        );
    }

    #[test]
    fn results_follow_the_order_of_the_files() {
        let files: Vec<PathBuf> = ["slow", "fast", "mid"].map(PathBuf::from).to_vec();
        let results = run_files(
            &files,
            3,
            NO_LIMITS,
            |file| {
                let name = file.to_string_lossy();
                let delay = if name == "slow" { "0.3" } else { "0" };
                let mut command = Command::new("sh");
                command.args([
                "-c",
                &format!(
                    "sleep {delay}; printf '\\001velyr-test {{\"type\":\"end\"}}\\n'; echo {name}"
                ),
            ]);
                command
            },
            |_| {},
        );
        let names: Vec<&str> = results.iter().map(|r| r.file.as_str()).collect();
        assert_eq!(names, ["slow", "fast", "mid"]);
        assert!(results.iter().all(|r| r.finished));
        assert_eq!(results[0].output, "slow\n");
    }

    #[test]
    fn a_command_that_cannot_start_is_a_failed_file() {
        let results = run_files(
            &[PathBuf::from("x")],
            1,
            NO_LIMITS,
            |_| Command::new("/nonexistent/velyr-test-child"),
            |_| {},
        );
        assert!(results[0].failed());
    }
}

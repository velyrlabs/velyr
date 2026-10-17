//! Spawning without V8: the environment is explicit, output is bounded, kill works, and a child does
//! not outlive the runtime that owns it. Every wait has a timeout so a regression fails, not hangs.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use velyr_io_process::{
    LimitedRead, Resolved, SpawnSpec, StdioMode, authorize_spawn, read_limited, run_blocking,
    spawn, wait_exit,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, ProgramPattern};

const LIMIT: Duration = Duration::from_secs(20);

fn resolve(program: &str) -> Resolved {
    let all = CapabilitySet::builder()
        .allow(Capability::ProcessSpawn(ProgramPattern::new("*")))
        .build();
    authorize_spawn(
        &all,
        program,
        Path::new("/"),
        Some(OsStr::new("/bin:/usr/bin")),
    )
    .unwrap()
}

fn spec(program: &str, args: &[&str], env: &[(&str, &str)]) -> SpawnSpec {
    SpawnSpec {
        arg0: program.into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: None,
        env: env
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect(),
        stdin: StdioMode::Null,
        stdout: StdioMode::Piped,
        stderr: StdioMode::Piped,
    }
}

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(LIMIT, future)
        .await
        .expect("timed out")
}

async fn stdout_of(program: &str, args: &[&str], env: &[(&str, &str)]) -> String {
    let mut process = spawn(&resolve(program), &spec(program, args, env)).unwrap();
    let mut out = String::new();
    within(process.stdout.as_mut().unwrap().read_to_string(&mut out))
        .await
        .unwrap();
    out
}

#[tokio::test(flavor = "current_thread")]
async fn nothing_is_inherited_from_the_host_environment() {
    // HOME exists in the test process; the child must not see it. (Not PATH: sh invents a default.)
    let script = r#"echo "${HOME-unset}:${X-unset}""#;
    assert_eq!(stdout_of("sh", &["-c", script], &[]).await, "unset:unset\n");
    assert_eq!(
        stdout_of("sh", &["-c", script], &[("X", "1")]).await,
        "unset:1\n"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_resolved_path_runs_whatever_path_the_child_is_given() {
    // The child's own PATH (empty) does not matter: the program is started by absolute path.
    let out = stdout_of("echo", &["hi"], &[("PATH", "")]).await;
    assert_eq!(out, "hi\n");
}

#[tokio::test(flavor = "current_thread")]
async fn argv0_is_the_name_the_caller_used() {
    let out = stdout_of("sh", &["-c", "echo $0"], &[]).await;
    assert_eq!(out, "sh\n");
}

#[tokio::test(flavor = "current_thread")]
async fn exit_codes_and_signals_are_reported() {
    let process = spawn(&resolve("sh"), &spec("sh", &["-c", "exit 7"], &[])).unwrap();
    let (_, exit) = process.handles();
    let exit = within(wait_exit(exit)).await;
    assert_eq!((exit.code, exit.signal), (Some(7), None));

    let process = spawn(&resolve("sleep"), &spec("sleep", &["7101"], &[])).unwrap();
    let (_, exit) = process.handles();
    process.kill();
    let exit = within(wait_exit(exit)).await;
    assert_eq!((exit.code, exit.signal), (None, Some(9)));
}

#[tokio::test(flavor = "current_thread")]
async fn an_endless_producer_is_cut_at_the_limit() {
    let script = "while :; do echo xxxxxxxx; done";
    let mut process = spawn(&resolve("sh"), &spec("sh", &["-c", script], &[])).unwrap();
    let started = Instant::now();
    let read = within(read_limited(process.stdout.as_mut().unwrap(), 4096))
        .await
        .unwrap();
    let LimitedRead::Overflow(kept) = read else {
        panic!("expected an overflow")
    };
    assert_eq!(kept.len(), 4096);
    // The caller stops the child; it does not linger blocked on a full pipe.
    let (_, exit) = process.handles();
    process.kill();
    assert_eq!(within(wait_exit(exit)).await.signal, Some(9));
    assert!(started.elapsed() < LIMIT);
}

fn alive(needle: &str) -> bool {
    let out = std::process::Command::new("/bin/ps")
        .args(["-axo", "stat=,command="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.contains(needle) && !line.trim_start().starts_with('Z'))
}

#[test]
fn a_child_dies_with_the_runtime_that_owns_it() {
    let needle = "sleep 7102";
    assert!(!alive(needle));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let process = spawn(&resolve("sleep"), &spec("sleep", &["7102"], &[])).unwrap();
        std::mem::forget(process);
    });
    assert!(
        alive(needle),
        "the child should be running while the runtime is"
    );
    drop(runtime);
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(needle) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(needle), "the child outlived its runtime");
}

#[test]
fn blocking_runs_feed_input_and_collect_output() {
    let mut cat = spec("cat", &[], &[]);
    cat.stdin = StdioMode::Piped;
    let run = run_blocking(
        &resolve("cat"),
        &cat,
        Some(b"round trip".to_vec()),
        1024,
        Some(LIMIT),
        &|| false,
    )
    .unwrap();
    assert_eq!(run.stdout, b"round trip");
    assert_eq!(
        (run.exit.code, run.timed_out, run.overflowed),
        (Some(0), false, false)
    );
}

#[test]
fn blocking_output_is_bounded_and_the_child_is_stopped() {
    let endless = spec("sh", &["-c", "while :; do echo xxxxxxxx; done"], &[]);
    let started = Instant::now();
    let run = run_blocking(&resolve("sh"), &endless, None, 2048, Some(LIMIT), &|| false).unwrap();
    assert!(run.overflowed && !run.timed_out);
    assert_eq!(run.stdout.len(), 2048);
    // SIGKILL, or SIGPIPE if the child wrote to the already-closed pipe first: either way it stopped.
    assert!(matches!(run.exit.signal, Some(9 | 13)), "{:?}", run.exit);
    assert!(started.elapsed() < LIMIT);
}

#[test]
fn blocking_timeout_kills_the_child_even_if_a_grandchild_keeps_the_pipe() {
    // `sh` is killed; its `sleep` child still holds stdout, which must not hold the caller.
    let held = spec("sh", &["-c", "sleep 7103; echo late"], &[]);
    let started = Instant::now();
    let run = run_blocking(
        &resolve("sh"),
        &held,
        None,
        1024,
        Some(Duration::from_millis(200)),
        &|| false,
    )
    .unwrap();
    assert!(run.timed_out);
    assert!(started.elapsed() < Duration::from_secs(5));
    let _ = std::process::Command::new("/usr/bin/pkill")
        .args(["-f", "sleep 7103"])
        .status();
}

#[test]
fn blocking_buffers_are_capped_whatever_the_caller_asks() {
    // `yes` fills the cap in well under a second; an unbounded Vec would keep growing.
    let endless = spec("yes", &[], &[]);
    let run = run_blocking(
        &resolve("yes"),
        &endless,
        None,
        usize::MAX,
        Some(Duration::from_secs(10)),
        &|| false,
    )
    .unwrap();
    assert!(run.overflowed && !run.timed_out, "{:?}", run.exit);
    assert_eq!(run.stdout.len(), velyr_io_process::MAX_BUFFER_LIMIT);
}

#[test]
fn a_blocking_run_ends_and_kills_the_child_when_cancelled() {
    let slept = spec("sleep", &["7109"], &[]);
    let started = Instant::now();
    let cancel_after = started + Duration::from_millis(200);
    let run = run_blocking(&resolve("sleep"), &slept, None, 1024, None, &|| {
        Instant::now() >= cancel_after
    })
    .unwrap();
    assert!(run.cancelled && !run.timed_out);
    assert_eq!(run.exit.signal, Some(9));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!alive("sleep 7109"));
}

#[test]
fn loader_variables_are_refused_in_the_child_environment() {
    use velyr_runtime_errors::ErrorCode;
    let options = |name: &str| {
        format!(
            r#"{{"program":"echo","args":[],"env":[["{name}","/tmp/x.so"]],"stdin":"null","stdout":"piped","stderr":"piped"}}"#
        )
    };
    for name in [
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_FRAMEWORK_PATH",
        "ld_preload",
    ] {
        let err = velyr_io_process::parse_options(&options(name)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PermissionDenied, "{name}");
        assert!(err.to_string().contains("loader"), "{name}: {err}");
    }
    assert!(velyr_io_process::parse_options(&options("LANG")).is_ok());
}

fn reap(needle: &str) {
    let _ = std::process::Command::new("/usr/bin/pkill")
        .args(["-f", needle])
        .status();
}

fn gone_soon(needle: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(needle) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    !alive(needle)
}

fn wait_alive(needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !alive(needle) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(alive(needle), "the grandchild never started");
}

#[test]
fn kill_takes_the_grandchildren_too() {
    let needle = "sleep 7201";
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let script = "sleep 7201 & wait";
        let process = spawn(&resolve("sh"), &spec("sh", &["-c", script], &[])).unwrap();
        let (_, exit) = process.handles();
        wait_alive(needle);
        process.kill();
        within(wait_exit(exit)).await;
    });
    let gone = gone_soon(needle);
    reap(needle);
    assert!(gone, "the grandchild outlived kill()");
}

#[test]
fn dropping_the_runtime_takes_the_grandchildren_too() {
    let needle = "sleep 7202";
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let process = spawn(
            &resolve("sh"),
            &spec("sh", &["-c", "sleep 7202 & wait"], &[]),
        )
        .unwrap();
        std::mem::forget(process);
    });
    wait_alive(needle);
    drop(runtime);
    let gone = gone_soon(needle);
    reap(needle);
    assert!(gone, "the grandchild outlived the runtime");
}

#[test]
fn blocking_timeout_takes_the_grandchildren_too() {
    let needle = "sleep 7203";
    let held = spec("sh", &["-c", "sleep 7203 & wait"], &[]);
    let run = run_blocking(
        &resolve("sh"),
        &held,
        None,
        1024,
        Some(Duration::from_millis(300)),
        &|| false,
    )
    .unwrap();
    assert!(run.timed_out);
    let gone = gone_soon(needle);
    reap(needle);
    assert!(gone, "the grandchild outlived the timeout");
}

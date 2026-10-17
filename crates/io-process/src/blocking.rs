//! Run a child to completion on the calling thread (`spawnSync`, `execSync`): feed it input,
//! collect stdout and stderr up to a limit each, and give up on a timeout. It goes through the same
//! gate and the same spawn settings as the async path: `Resolved` in, nothing inherited.

use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::gate::Resolved;
use crate::process::{Exit, MAX_BUFFER_LIMIT, SpawnSpec, StdioMode, kill_group};

const POLL: Duration = Duration::from_millis(2);
/// How long to wait for a pipe to reach EOF once the child is gone: a grandchild that inherited it
/// must not hold the caller forever.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// What a blocking run produced. `stdout` / `stderr` are empty unless that stream was piped.
#[derive(Debug)]
pub struct BlockingOutput {
    pub exit: Exit,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    /// A piped stream went over `max_buffer`: the child was killed and the output is cut there.
    pub overflowed: bool,
    /// `cancelled` fired: the child was killed because the caller is going away.
    pub cancelled: bool,
}

fn stdio(mode: StdioMode) -> Stdio {
    match mode {
        StdioMode::Piped => Stdio::piped(),
        StdioMode::Inherit => Stdio::inherit(),
        StdioMode::Null => Stdio::null(),
    }
}

/// One stream being read on its own thread.
#[derive(Default)]
struct Sink {
    data: Mutex<Vec<u8>>,
    done: AtomicBool,
}

fn drain(
    mut stream: impl Read + Send + 'static,
    max: usize,
    overflow: Arc<AtomicBool>,
) -> Arc<Sink> {
    let sink = Arc::new(Sink::default());
    let shared = sink.clone();
    thread::spawn(move || {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let n = match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut data = shared.data.lock().expect("sink");
            let room = max - data.len();
            data.extend_from_slice(&chunk[..n.min(room)]);
            if n > room {
                overflow.store(true, Ordering::Release);
                break;
            }
        }
        shared.done.store(true, Ordering::Release);
    });
    sink
}

fn finish(sink: Option<Arc<Sink>>, until: Instant) -> Vec<u8> {
    let Some(sink) = sink else { return Vec::new() };
    while !sink.done.load(Ordering::Acquire) && Instant::now() < until {
        thread::sleep(POLL);
    }
    std::mem::take(&mut *sink.data.lock().expect("sink"))
}

/// Runs `resolved` with `spec`. `input` is written to a piped stdin, which is then closed.
/// `max_buffer` is clamped to [`MAX_BUFFER_LIMIT`]. `cancelled` is polled while waiting: once it
/// returns true (the isolate is being terminated) the child is killed and the run ends with
/// `cancelled` set, so a blocked call cannot hold the isolate's thread.
pub fn run_blocking(
    resolved: &Resolved,
    spec: &SpawnSpec,
    input: Option<Vec<u8>>,
    max_buffer: usize,
    timeout: Option<Duration>,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<BlockingOutput> {
    let max_buffer = max_buffer.min(MAX_BUFFER_LIMIT);
    let mut command = Command::new(resolved.path());
    command
        .arg0(&spec.arg0)
        .args(&spec.args)
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(stdio(spec.stdin))
        .stdout(stdio(spec.stdout))
        .stderr(stdio(spec.stderr))
        .process_group(0);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn()?;
    let overflow = Arc::new(AtomicBool::new(false));
    let out = child
        .stdout
        .take()
        .map(|s| drain(s, max_buffer, overflow.clone()));
    let err = child
        .stderr
        .take()
        .map(|s| drain(s, max_buffer, overflow.clone()));
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.unwrap_or_default();
        thread::spawn(move || {
            // A child that exits without reading its input is not an error here.
            let _ = stdin.write_all(&input);
        });
    }
    let deadline = timeout.map(|t| Instant::now() + t);
    let mut timed_out = false;
    let mut was_cancelled = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(e) => {
                kill_group(child.id());
                let _ = child.kill();
                break Err(e);
            }
        }
        let late = deadline.is_some_and(|d| Instant::now() >= d);
        was_cancelled = cancelled();
        if late || was_cancelled || overflow.load(Ordering::Acquire) {
            timed_out = late;
            kill_group(child.id());
            let _ = child.kill();
            break child.wait();
        }
        thread::sleep(POLL);
    };
    let until = Instant::now() + DRAIN_GRACE;
    Ok(BlockingOutput {
        exit: status?.into(),
        stdout: finish(out, until),
        stderr: finish(err, until),
        timed_out,
        overflowed: overflow.load(Ordering::Acquire),
        cancelled: was_cancelled,
    })
}

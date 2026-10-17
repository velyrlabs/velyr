//! Spawning and supervising one child. A child is owned by a single supervisor task that waits for
//! it or kills it on request, so `wait` and `kill` never contend for the `Child`. The child is
//! spawned with `kill_on_drop`, so it dies with the task: when the isolate's runtime ends, nothing
//! is left running. Every child leads its own process group, and kill, timeout and drop signal the
//! whole group, so grandchildren die with it.

use std::ffi::OsString;
use std::io;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};

#[cfg(unix)]
use nix::sys::signal::{Signal, killpg};
#[cfg(unix)]
use nix::unistd::Pid;
use tokio::io::AsyncReadExt;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::gate::Resolved;

/// The most output kept per stream, wherever a `maxBuffer` is taken (64 MiB). Callers that ask for
/// more are refused, and the readers clamp to it regardless, so no request can grow a buffer
/// without bound.
pub const MAX_BUFFER_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioMode {
    Piped,
    Inherit,
    Null,
}

impl StdioMode {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "piped" => Some(Self::Piped),
            "inherit" => Some(Self::Inherit),
            "null" => Some(Self::Null),
            _ => None,
        }
    }

    fn stdio(self) -> Stdio {
        match self {
            Self::Piped => Stdio::piped(),
            Self::Inherit => Stdio::inherit(),
            Self::Null => Stdio::null(),
        }
    }
}

/// What to run. `env` is the child's whole environment: nothing is inherited.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    /// `argv[0]`: the program as the caller named it.
    pub arg0: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(OsString, OsString)>,
    pub stdin: StdioMode,
    pub stdout: StdioMode,
    pub stderr: StdioMode,
}

/// How a child ended: an exit code, or the signal that killed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl From<ExitStatus> for Exit {
    fn from(status: ExitStatus) -> Self {
        Self {
            code: status.code(),
            #[cfg(unix)]
            signal: status.signal(),
            #[cfg(not(unix))]
            signal: None,
        }
    }
}

/// SIGKILLs the process group led by `pid`. Does nothing for pid 0 (`killpg(0)` would hit our own
/// group); a group that is already gone is not an error.
#[cfg(unix)]
pub(crate) fn kill_group(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else { return };
    if pid > 0 {
        let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
    }
}

/// Nothing to signal off unix: `spawn` never starts a child there.
#[cfg(not(unix))]
pub(crate) fn kill_group(_pid: u32) {}

struct GroupGuard {
    pid: u32,
    armed: bool,
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.armed {
            kill_group(self.pid);
        }
    }
}

/// A running child: its pipes, a kill switch and the exit status once there is one.
pub struct Process {
    pub pid: u32,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    kill: CancellationToken,
    exit: watch::Receiver<Option<Exit>>,
}

/// Starts `resolved` with `spec`. Needs a Tokio runtime context.
pub fn spawn(resolved: &Resolved, spec: &SpawnSpec) -> io::Result<Process> {
    if cfg!(not(unix)) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "spawning processes is not supported on this platform",
        ));
    }
    let mut command = Command::new(resolved.path());
    command
        .args(&spec.args)
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(spec.stdin.stdio())
        .stdout(spec.stdout.stdio())
        .stderr(spec.stderr.stdio())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.arg0(&spec.arg0).process_group(0);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn()?;
    let pid = child.id().unwrap_or(0);
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let kill = CancellationToken::new();
    let (tx, exit) = watch::channel(None);
    let on_kill = kill.clone();
    // Armed while the child runs: if the task is dropped, even unpolled (runtime ending), the group dies.
    let guard = GroupGuard { pid, armed: true };
    tokio::spawn(async move {
        let mut guard = guard; // move the whole guard in, not just its field
        let status = tokio::select! {
            status = child.wait() => status,
            () = on_kill.cancelled() => {
                kill_group(pid);
                match child.kill().await {
                    Ok(()) => child.wait().await,
                    Err(err) => Err(err),
                }
            },
        };
        guard.armed = false;
        // The pipes were taken from `child`; a failed wait still ends the process's story.
        let exit = status.map(Exit::from).unwrap_or(Exit {
            code: None,
            signal: None,
        });
        let _ = tx.send(Some(exit));
    });
    Ok(Process {
        pid,
        stdin,
        stdout,
        stderr,
        kill,
        exit,
    })
}

impl Process {
    /// SIGKILL. Harmless once the child has ended.
    pub fn kill(&self) {
        self.kill.cancel();
    }

    /// Kill switch and exit watcher for code that moves the pipes elsewhere.
    pub fn handles(&self) -> (CancellationToken, watch::Receiver<Option<Exit>>) {
        (self.kill.clone(), self.exit.clone())
    }
}

/// Waits for the exit published by a supervisor.
pub async fn wait_exit(mut exit: watch::Receiver<Option<Exit>>) -> Exit {
    loop {
        if let Some(done) = *exit.borrow_and_update() {
            return done;
        }
        if exit.changed().await.is_err() {
            // The supervisor ended without publishing (its runtime is going away).
            return Exit {
                code: None,
                signal: None,
            };
        }
    }
}

/// Why [`read_limited`] stopped.
#[derive(Debug)]
pub enum LimitedRead {
    Done(Vec<u8>),
    /// More than `max` bytes arrived; the first `max` are kept.
    Overflow(Vec<u8>),
}

/// Reads `stream` to EOF, keeping at most `max` bytes. Memory is bounded by `max` plus one chunk.
pub async fn read_limited<R: tokio::io::AsyncRead + Unpin>(
    stream: &mut R,
    max: usize,
) -> io::Result<LimitedRead> {
    let max = max.min(MAX_BUFFER_LIMIT);
    let mut data = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(LimitedRead::Done(data));
        }
        let room = max - data.len();
        if n > room {
            data.extend_from_slice(&chunk[..room]);
            return Ok(LimitedRead::Overflow(data));
        }
        data.extend_from_slice(&chunk[..n]);
    }
}

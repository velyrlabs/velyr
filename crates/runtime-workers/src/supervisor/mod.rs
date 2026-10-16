//! The multicore supervisor behind `velyr serve --workers=N` (ADR-014).
//!
//! It runs on its own thread, starts `N` server workers (internal isolates, not JS `Worker`s;
//! what a worker runs is the caller's closure) and spreads connections over them:
//!
//! - [`Distribution::ReusePort`] (Linux, FreeBSD): each worker binds the address itself;
//! - [`Distribution::Handoff`] (macOS, Windows): an acceptor thread passes each connection to a
//!   worker through a channel (`WorkerContext::streams`).
//!
//! A worker that returns an error or panics is restarted with backoff, at most
//! `max_restarts` times per `window`; past that the supervisor stops everything and ends with
//! [`SupervisorError::TooManyRestarts`]. A worker that returns `Ok` is finished for good.
//! Checking the `net-listen` permission for the address is the caller's job, done once before
//! [`Supervisor::start`].

use std::fmt;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

mod handoff;
mod restart;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod reuseport;

use handoff::Slot;
pub use restart::RestartPolicy;

/// How connections reach the workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distribution {
    /// The supervisor accepts and hands streams to workers.
    Handoff,
    /// Every worker binds the address with `SO_REUSEPORT` (Linux and FreeBSD only).
    ReusePort,
}

impl Distribution {
    /// The strategy ADR-014 picks for this platform.
    pub fn native() -> Self {
        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            Self::ReusePort
        } else {
            Self::Handoff
        }
    }
}

#[derive(Debug, Clone)]
pub struct SupervisorOptions {
    pub workers: usize,
    /// Address to serve; port 0 picks a free port, reported by [`Supervisor::addr`].
    pub addr: SocketAddr,
    pub distribution: Distribution,
    /// Restarts allowed per `window` before the supervisor gives up.
    pub max_restarts: u32,
    pub window: Duration,
    /// First restart delay; it doubles with each failure in the window (capped at 2 s).
    pub backoff_base: Duration,
}

impl SupervisorOptions {
    pub fn new(workers: usize, addr: SocketAddr) -> Self {
        Self {
            workers,
            addr,
            distribution: Distribution::native(),
            max_restarts: 5,
            window: Duration::from_secs(60),
            backoff_base: Duration::from_millis(100),
        }
    }
}

/// What one server worker gets to run with.
pub struct WorkerContext {
    /// `0..count`.
    pub id: usize,
    pub count: usize,
    /// The address being served (the resolved one when port 0 was asked).
    pub addr: SocketAddr,
    /// Connections for this worker; `Some` with [`Distribution::Handoff`].
    pub streams: Option<tokio::sync::mpsc::Receiver<std::net::TcpStream>>,
    /// Cancelled when the supervisor stops; the worker should wind down.
    pub shutdown: CancellationToken,
}

#[derive(Debug)]
pub enum SupervisorError {
    Bind(io::Error),
    TooManyRestarts { limit: u32, last: String },
    Setup(String),
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind(err) => write!(f, "cannot listen: {err}"),
            Self::TooManyRestarts { limit, last } => {
                write!(
                    f,
                    "server workers failed more than {limit} times in a minute; last: {last}"
                )
            }
            Self::Setup(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SupervisorError {}

struct Shared {
    shutdown: CancellationToken,
    stopping: AtomicBool,
    restarts: AtomicU32,
    slots: Vec<Arc<Slot>>,
    /// Address the handoff acceptor listens on.
    acceptor: Option<SocketAddr>,
}

impl Shared {
    fn stop(&self) {
        self.shutdown.cancel();
        self.stop_accepting();
    }

    /// Closes the handoff channels and the acceptor; workers keep running what they have.
    fn stop_accepting(&self) {
        if !self.stopping.swap(true, Ordering::AcqRel) {
            self.slots.iter().for_each(|slot| slot.close());
            if let Some(addr) = self.acceptor {
                handoff::wake(addr);
            }
        }
    }
}

struct Exit {
    id: usize,
    result: Result<(), String>,
}

/// A cloneable handle to a running [`Supervisor`], for the thread that reacts to signals while
/// another one waits in [`Supervisor::join`].
#[derive(Clone)]
pub struct SupervisorHandle(Arc<Shared>);

impl SupervisorHandle {
    /// Stops handing out new connections; workers drain what they already have.
    pub fn stop_accepting(&self) {
        self.0.stop_accepting();
    }

    /// Same as [`Supervisor::shutdown`].
    pub fn shutdown(&self) {
        self.0.stop();
    }
}

pub struct Supervisor {
    addr: SocketAddr,
    shared: Arc<Shared>,
    thread: JoinHandle<Result<(), SupervisorError>>,
}

impl Supervisor {
    /// Binds the address, then starts the supervisor thread and the workers. `run` is each
    /// worker's body: it runs on a thread of its own and blocks until the worker is done.
    pub fn start<F>(options: SupervisorOptions, run: F) -> Result<Self, SupervisorError>
    where
        F: Fn(WorkerContext) -> Result<(), String> + Send + Sync + 'static,
    {
        let count = options.workers.max(1);
        let slots: Vec<Arc<Slot>> = (0..count).map(|_| Arc::new(Slot::default())).collect();
        let (addr, listener, _holder) = bind(&options)?;
        let shared = Arc::new(Shared {
            shutdown: CancellationToken::new(),
            stopping: AtomicBool::new(false),
            restarts: AtomicU32::new(0),
            slots,
            acceptor: listener.as_ref().map(|_| addr),
        });
        let run = Arc::new(run);
        let (exit_tx, exit_rx) = mpsc::channel();
        let handoff = options.distribution == Distribution::Handoff;
        // Channels exist before any connection can arrive, so the first round is an even spread.
        let streams: Vec<_> = shared
            .slots
            .iter()
            .map(|slot| handoff.then(|| slot.open()))
            .collect();
        let acceptor = listener
            .map(|listener| {
                let (slots, shared) = (shared.slots.clone(), shared.clone());
                thread::Builder::new()
                    .name("velyr-serve-acceptor".into())
                    .spawn(move || handoff::accept_loop(&listener, &slots, &shared.stopping))
            })
            .transpose()
            .map_err(|err| SupervisorError::Setup(err.to_string()))?;
        let launcher = Launcher {
            shared: shared.clone(),
            run,
            exits: exit_tx,
            addr,
            count,
        };
        let policy = RestartPolicy::new(options.max_restarts, options.window, options.backoff_base);
        let thread = thread::Builder::new()
            .name("velyr-supervisor".into())
            .spawn(move || {
                let _holder = _holder;
                let result = supervise(&launcher, streams, exit_rx, policy);
                launcher.shared.stop();
                if let Some(acceptor) = acceptor {
                    let _ = acceptor.join();
                }
                result
            })
            .map_err(|err| SupervisorError::Setup(err.to_string()))?;
        Ok(Self {
            addr,
            shared,
            thread,
        })
    }

    /// The address being served.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Workers restarted so far.
    pub fn restarts(&self) -> u32 {
        self.shared.restarts.load(Ordering::Relaxed)
    }

    pub fn handle(&self) -> SupervisorHandle {
        SupervisorHandle(self.shared.clone())
    }

    /// Tells every worker to wind down and stops accepting.
    pub fn shutdown(&self) {
        self.shared.stop();
    }

    /// Waits for the supervisor thread: every worker finished, or it gave up.
    pub fn join(self) -> Result<(), SupervisorError> {
        self.thread
            .join()
            .unwrap_or_else(|_| Err(SupervisorError::Setup("supervisor thread panicked".into())))
    }
}

type Holder = Option<Box<dyn std::any::Any + Send>>;

fn bind(
    options: &SupervisorOptions,
) -> Result<(SocketAddr, Option<TcpListener>, Holder), SupervisorError> {
    match options.distribution {
        Distribution::Handoff => {
            let listener = TcpListener::bind(options.addr).map_err(SupervisorError::Bind)?;
            let addr = listener.local_addr().map_err(SupervisorError::Bind)?;
            Ok((addr, Some(listener), None))
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        Distribution::ReusePort => {
            let (socket, addr) = reuseport::hold(options.addr).map_err(SupervisorError::Bind)?;
            Ok((addr, None, Some(Box::new(socket))))
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        Distribution::ReusePort => Err(SupervisorError::Bind(io::Error::other(
            "SO_REUSEPORT distribution is not supported on this platform",
        ))),
    }
}

struct Launcher<F> {
    shared: Arc<Shared>,
    run: Arc<F>,
    exits: mpsc::Sender<Exit>,
    addr: SocketAddr,
    count: usize,
}

impl<F> Launcher<F>
where
    F: Fn(WorkerContext) -> Result<(), String> + Send + Sync + 'static,
{
    /// Starts worker `id` after `delay`. With a delay its channel opens only once it is about
    /// to run, so connections are not queued for a worker that is still backing off.
    fn launch(
        &self,
        id: usize,
        delay: Duration,
        streams: Option<tokio::sync::mpsc::Receiver<std::net::TcpStream>>,
    ) {
        let (shared, run, exits) = (self.shared.clone(), self.run.clone(), self.exits.clone());
        let (addr, count) = (self.addr, self.count);
        let handoff = shared.acceptor.is_some();
        let spawned = thread::Builder::new()
            .name(format!("velyr-serve-worker-{id}"))
            .spawn(move || {
                let result = if shared.shutdown.is_cancelled() {
                    Ok(())
                } else {
                    let streams = streams.or_else(|| {
                        thread::sleep(delay);
                        handoff.then(|| shared.slots[id].open())
                    });
                    let ctx = WorkerContext {
                        id,
                        count,
                        addr,
                        streams,
                        shutdown: shared.shutdown.clone(),
                    };
                    catch_unwind(AssertUnwindSafe(|| run(ctx)))
                        .unwrap_or_else(|panic| Err(describe(&panic)))
                };
                let _ = exits.send(Exit { id, result });
            });
        if let Err(err) = spawned {
            let _ = self.exits.send(Exit {
                id,
                result: Err(format!("cannot start the thread: {err}")),
            });
        }
    }
}

fn describe(panic: &Box<dyn std::any::Any + Send>) -> String {
    let message = panic
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into());
    format!("panicked: {message}")
}

fn supervise<F>(
    launcher: &Launcher<F>,
    streams: Vec<Option<tokio::sync::mpsc::Receiver<std::net::TcpStream>>>,
    exits: mpsc::Receiver<Exit>,
    mut policy: RestartPolicy,
) -> Result<(), SupervisorError>
where
    F: Fn(WorkerContext) -> Result<(), String> + Send + Sync + 'static,
{
    let mut live = streams.len();
    for (id, streams) in streams.into_iter().enumerate() {
        launcher.launch(id, Duration::ZERO, streams);
    }
    let mut failure = None;
    while live > 0 {
        let Ok(Exit { id, result }) = exits.recv() else {
            break;
        };
        launcher.shared.slots[id].close();
        let Err(message) = result else {
            live -= 1;
            continue;
        };
        if launcher.shared.shutdown.is_cancelled() {
            live -= 1;
            continue;
        }
        tracing::warn!(worker = id, error = %message, "server worker failed");
        match policy.record(Instant::now()) {
            Some(backoff) => {
                launcher.shared.restarts.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(worker = id, ?backoff, "restarting server worker");
                launcher.launch(id, backoff, None);
            }
            None => {
                live -= 1;
                failure.get_or_insert(message);
                launcher.shared.stop();
            }
        }
    }
    match failure {
        Some(last) => Err(SupervisorError::TooManyRestarts {
            limit: policy.limit(),
            last,
        }),
        None => Ok(()),
    }
}

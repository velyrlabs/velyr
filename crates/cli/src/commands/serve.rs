//! `velyr serve <file>`: runs the module and serves its `export default { fetch }` with
//! `Velyr.serve`. One worker runs in this process's isolate as for `velyr run`; with
//! `--workers=N` (or `workers` in `velyr.json`) a supervisor runs N isolates instead.

use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use velyr_runtime_permissions::Request;
use velyr_runtime_workers::shutdown::{DEFAULT_DEADLINE, Drain};
use velyr_runtime_workers::supervisor::{
    Distribution, Supervisor, SupervisorHandle, SupervisorOptions, WorkerContext,
};
use velyr_web_fetch::ServeInbox;

use crate::config::Workers;
use crate::permissions::Permissions;
use crate::run::{self, Worker};

/// How the started server learns about its address and its peers.
enum Listen<'a> {
    /// Bind `hostname:port` and print where it listens.
    Alone { hostname: &'a str, port: u16 },
    /// Serve what the supervisor hands over (or binds for it); `id` is the worker's number.
    Worker { addr: SocketAddr, id: usize },
}

/// How much longer than the shutdown deadline a server waits for its connections.
const GRACE_MARGIN: Duration = Duration::from_secs(5);

/// Starts the module's `fetch` handler. Runs once the isolate is set up, so `Velyr.serve` and
/// the Fetch classes exist; the module itself is the entry, so its imports resolve as for `run`.
fn start_script(file: &Path, listen: &Listen<'_>, deadline: Duration) -> String {
    // The server's own grace must outlast the deadline: when both expire together the server
    // gives up first, the event loop ends with 0, and the forced exit (1) is lost.
    let grace_ms = (deadline + GRACE_MARGIN).as_millis();
    let specifier = serde_json::Value::from(file.to_string_lossy().into_owned());
    let (options, handler) = match listen {
        Listen::Alone { hostname, port } => (
            format!(
                r#"gracePeriod: {grace_ms}, hostname: {}, port: {port}, onListen: ({{ hostname, port }}) => console.error(`Listening on http://${{hostname}}:${{port}}/`)"#,
                serde_json::Value::from(*hostname)
            ),
            "app.fetch(request, info)".to_owned(),
        ),
        Listen::Worker { addr, id } => (
            format!(
                "gracePeriod: {grace_ms}, hostname: {}, port: {}, inherit: true",
                serde_json::Value::from(addr.ip().to_string()),
                addr.port()
            ),
            format!("app.fetch(request, {{ ...info, workerId: {id} }})"),
        ),
    };
    // A timer, because `import()` only works once the module host is installed, after this runs.
    format!(
        r#"setTimeout(() => import({specifier}).then((module) => {{
  const app = module.default;
  if (typeof app?.fetch !== "function") {{
    throw new TypeError("velyr serve: the module must export default {{ fetch }}");
  }}
  Velyr.serve({{ {options} }}, (request, info) => {handler});
}}), 0);"#
    )
}

/// What the signal thread drives: the supervisor of a multicore server, or nothing (one isolate).
enum Target {
    Single,
    Supervised(SupervisorHandle, Drain),
}

/// First SIGINT/SIGTERM: stop accepting and drain (servers close their listeners at once, and
/// everything has `deadline` to finish). Past the deadline the shutdown is forced and the exit
/// code is 1. A second signal stops the process right away.
fn stop_on_signals(target: Target, deadline: Duration) {
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async {
            let mut signals = Signals::new();
            signals.next().await;
            eprintln!("shutting down, send the signal again to stop now");
            velyr_web_fetch::termination_token().cancel();
            let drain = async {
                match &target {
                    Target::Supervised(handle, drain) => drain.run(handle, deadline).await,
                    Target::Single => {
                        tokio::time::sleep(deadline).await;
                        eprintln!("shutdown deadline passed, terminating");
                        std::process::exit(1);
                    }
                }
            };
            tokio::select! {
                () = signals.next() => std::process::exit(130),
                () = drain => {}
            }
            signals.next().await;
            std::process::exit(130);
        });
    });
}

/// `VELYR_SHUTDOWN_DEADLINE_MS`, default 10 s: how long a signal-triggered shutdown may drain.
fn shutdown_deadline() -> Result<Duration, String> {
    match std::env::var("VELYR_SHUTDOWN_DEADLINE_MS") {
        Err(_) => Ok(DEFAULT_DEADLINE),
        Ok(text) => text
            .parse()
            .map(Duration::from_millis)
            .map_err(|_| "VELYR_SHUTDOWN_DEADLINE_MS must be a number of milliseconds".to_owned()),
    }
}

#[cfg(unix)]
struct Signals(tokio::signal::unix::Signal, tokio::signal::unix::Signal);

#[cfg(unix)]
impl Signals {
    fn new() -> Self {
        use tokio::signal::unix::{SignalKind, signal};
        Self(
            signal(SignalKind::interrupt()).expect("SIGINT handler"),
            signal(SignalKind::terminate()).expect("SIGTERM handler"),
        )
    }

    async fn next(&mut self) {
        tokio::select! { _ = self.0.recv() => {}, _ = self.1.recv() => {} }
    }
}

#[cfg(not(unix))]
struct Signals;

#[cfg(not(unix))]
impl Signals {
    fn new() -> Self {
        Self
    }

    async fn next(&mut self) {
        let _ = tokio::signal::ctrl_c().await;
    }
}

pub fn serve(
    file: &Path,
    hostname: &str,
    port: u16,
    workers: Option<Workers>,
    flags: Permissions,
) -> ExitCode {
    let _span = tracing::info_span!("velyr.cli.serve").entered();
    let file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let count = match worker_count(workers) {
        Ok(count) => count,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    let deadline = match shutdown_deadline() {
        Ok(deadline) => deadline,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    if count == 1 {
        stop_on_signals(Target::Single, deadline);
        let script = start_script(&file, &Listen::Alone { hostname, port }, deadline);
        return run::run_with(&file, flags, Some(script));
    }
    match serve_multicore(&file, hostname, port, count, flags, deadline) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The flag wins over `velyr.json`; without either there is one worker.
fn worker_count(flag: Option<Workers>) -> Result<usize, String> {
    let config = run::load_config()?.map(|loaded| loaded.config);
    let memory_mb = config
        .as_ref()
        .and_then(|config| config.quotas.as_ref()?.memory_mb);
    let configured = flag.or_else(|| config.and_then(|config| config.workers));
    configured.map_or(Ok(1), |workers| workers.count(memory_mb))
}

fn serve_multicore(
    file: &Path,
    hostname: &str,
    port: u16,
    count: usize,
    flags: Permissions,
    deadline: Duration,
) -> Result<ExitCode, String> {
    let prepared = run::prepare(file, flags)?;
    let ip = velyr_web_fetch::resolve_listen_host(hostname).map_err(|err| err.to_string())?;
    // Checked once for the address; every worker runs with the same authority.
    prepared
        .capabilities
        .check(&Request::NetListen(&ip.to_string(), port))
        .into_result()
        .map_err(|err| err.to_string())?;
    let options = SupervisorOptions::new(count, SocketAddr::new(ip, port));
    let reuse_port = options.distribution == Distribution::ReusePort;
    let file = file.to_path_buf();
    let supervisor = Supervisor::start(options, move |ctx: WorkerContext| {
        velyr_runtime_core::metrics::global().register_worker(ctx.id);
        let script = start_script(
            &file,
            &Listen::Worker {
                addr: ctx.addr,
                id: ctx.id,
            },
            deadline,
        );
        let inbox = ServeInbox {
            addr: ctx.addr,
            streams: ctx.streams,
            reuse_port,
        };
        let worker = Worker {
            inbox: Some(inbox),
            shutdown: Some(ctx.shutdown.clone()),
        };
        let result = run::run_prepared(&file, &prepared, Some(script), worker);
        match (result, ctx.shutdown.is_cancelled()) {
            (_, true) => Ok(()),
            // SIGINT/SIGTERM drained this worker's servers: finishing is the expected exit.
            (Ok(()), false) if velyr_web_fetch::termination_token().is_cancelled() => Ok(()),
            // A server worker that returns on its own stopped serving: restart it.
            (Ok(()), false) => Err("the worker stopped serving".to_owned()),
            (Err(message), false) => Err(message),
        }
    })
    .map_err(|err| err.to_string())?;
    eprintln!("Listening on http://{}/", supervisor.addr());
    let drain = Drain::default();
    stop_on_signals(
        Target::Supervised(supervisor.handle(), drain.clone()),
        deadline,
    );
    let joined = supervisor.join().map_err(|err| err.to_string());
    drain.finished();
    joined?;
    Ok(if drain.was_forced() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

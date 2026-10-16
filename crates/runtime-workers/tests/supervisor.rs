//! The multicore supervisor, with plain closures as workers (no engine): connection handoff,
//! restarts with backoff and the restart budget.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use velyr_runtime_workers::supervisor::{
    Distribution, RestartPolicy, Supervisor, SupervisorError, SupervisorOptions, WorkerContext,
};

fn local() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn options(workers: usize) -> SupervisorOptions {
    let mut options = SupervisorOptions::new(workers, local());
    options.distribution = Distribution::Handoff;
    options.backoff_base = Duration::from_millis(1);
    options
}

/// Answers every handed-over connection with the worker id, then closes it.
fn id_worker(mut ctx: WorkerContext) -> Result<(), String> {
    let mut streams = ctx.streams.take().expect("handoff gives streams");
    while let Some(stream) = streams.blocking_recv() {
        use std::io::Write;
        let mut stream = stream;
        let _ = stream.write_all(ctx.id.to_string().as_bytes());
    }
    Ok(())
}

fn ask(addr: SocketAddr) -> Option<usize> {
    let mut stream = TcpStream::connect(addr).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut text = String::new();
    stream.read_to_string(&mut text).ok()?;
    text.parse().ok()
}

#[test]
fn restart_policy_allows_five_per_minute_then_gives_up() {
    let mut policy = RestartPolicy::new(5, Duration::from_secs(60), Duration::from_millis(100));
    let start = Instant::now();
    let backoffs: Vec<_> = (0..5)
        .map(|i| policy.record(start + Duration::from_secs(i)).unwrap())
        .collect();
    assert_eq!(backoffs[0], Duration::from_millis(100));
    assert_eq!(backoffs[1], Duration::from_millis(200));
    assert!(backoffs.windows(2).all(|pair| pair[1] >= pair[0]));
    assert!(backoffs.iter().all(|b| *b <= Duration::from_secs(2)));
    assert_eq!(policy.record(start + Duration::from_secs(6)), None);
}

#[test]
fn restart_policy_forgets_failures_older_than_the_window() {
    let mut policy = RestartPolicy::new(2, Duration::from_secs(60), Duration::from_millis(100));
    let start = Instant::now();
    assert!(policy.record(start).is_some());
    assert!(policy.record(start + Duration::from_secs(1)).is_some());
    assert!(policy.record(start + Duration::from_secs(2)).is_none());
    assert_eq!(
        policy.record(start + Duration::from_secs(120)),
        Some(Duration::from_millis(100))
    );
}

#[test]
fn handoff_spreads_connections_round_robin_over_the_workers() {
    let supervisor = Supervisor::start(options(3), id_worker).unwrap();
    let addr = supervisor.addr();
    let mut seen: BTreeMap<usize, usize> = BTreeMap::new();
    for _ in 0..30 {
        *seen.entry(ask(addr).expect("answered")).or_default() += 1;
    }
    assert_eq!(seen, BTreeMap::from([(0, 10), (1, 10), (2, 10)]));
    supervisor.shutdown();
    supervisor.join().unwrap();
}

#[test]
fn a_failed_worker_is_restarted_and_the_service_continues() {
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = runs.clone();
    let supervisor = Supervisor::start(options(2), move |ctx| {
        if ctx.id == 1 && counter.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err("boom".into());
        }
        id_worker(ctx)
    })
    .unwrap();
    let addr = supervisor.addr();
    let deadline = Instant::now() + Duration::from_secs(5);
    while supervisor.restarts() < 1 {
        assert!(Instant::now() < deadline, "no restart");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Give the new worker a moment to register, then both must answer again.
    let mut ids = std::collections::BTreeSet::new();
    while ids.len() < 2 && Instant::now() < deadline {
        ids.extend(ask(addr));
    }
    assert_eq!(ids, [0, 1].into());
    assert_eq!(supervisor.restarts(), 1);
    supervisor.shutdown();
    supervisor.join().unwrap();
}

#[test]
fn a_worker_that_panics_counts_as_a_failure() {
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = runs.clone();
    let supervisor = Supervisor::start(options(1), move |ctx| {
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("worker crashed");
        }
        id_worker(ctx)
    })
    .unwrap();
    let addr = supervisor.addr();
    let deadline = Instant::now() + Duration::from_secs(5);
    while supervisor.restarts() < 1 {
        assert!(Instant::now() < deadline, "no restart");
        std::thread::sleep(Duration::from_millis(5));
    }
    while ask(addr).is_none() {
        assert!(Instant::now() < deadline, "no service");
    }
    supervisor.shutdown();
    supervisor.join().unwrap();
}

#[test]
fn too_many_failures_end_the_supervisor() {
    let supervisor = Supervisor::start(options(1), |_ctx| Err("always".into())).unwrap();
    let result = supervisor.join();
    assert!(
        matches!(result, Err(SupervisorError::TooManyRestarts { .. })),
        "{result:?}"
    );
}

#[test]
fn workers_that_finish_normally_end_the_supervisor_without_restarts() {
    let supervisor = Supervisor::start(options(2), |_ctx| Ok(())).unwrap();
    assert!(supervisor.join().is_ok());
}

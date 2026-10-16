//! `velyr serve --workers=N`: the supervisor spreads connections over N isolates and restarts
//! the ones that crash.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn fixture(name: &str) -> String {
    format!(
        "{}/../../tests/fixtures/m5-serve/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// A running `velyr serve`; its stderr is read by a thread so the pipe never fills.
struct Serve {
    child: Child,
    port: u16,
    stderr: mpsc::Receiver<String>,
    log: String,
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Serve {
    fn start(args: &[&str], dir: Option<&std::path::Path>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_velyr"));
        command
            .arg("serve")
            .args([
                "--allow-listen=127.0.0.1:0",
                "--hostname",
                "127.0.0.1",
                "--port",
                "0",
            ])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(dir) = dir {
            command.current_dir(dir);
        }
        let mut child = command.spawn().unwrap();
        let (tx, stderr) = mpsc::channel();
        let pipe = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let mut serve = Self {
            child,
            port: 0,
            stderr,
            log: String::new(),
        };
        let line = serve.wait_for("Listening on http://");
        let port = line.rsplit(':').next().unwrap().trim_end_matches('/');
        serve.port = port.parse().unwrap();
        serve
    }

    /// Waits for a stderr line containing `needle`.
    fn wait_for(&mut self, needle: &str) -> String {
        if let Some(line) = self.log.lines().find(|line| line.contains(needle)) {
            return line.to_owned();
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while let Ok(line) = self
            .stderr
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            self.log.push_str(&line);
            self.log.push('\n');
            if line.contains(needle) {
                return line;
            }
        }
        panic!("no stderr line with {needle:?}; saw:\n{}", self.log);
    }

    fn get(&self, path: &str) -> Option<String> {
        get(self.port, path)
    }

    fn worker_ids(&self, requests: usize) -> BTreeSet<String> {
        (0..requests).filter_map(|_| self.get("/")).collect()
    }
}

/// The body of `GET path`, or `None` if the connection failed.
fn get(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response
        .starts_with("HTTP/1.1 200")
        .then(|| response.rsplit("\r\n\r\n").next().unwrap().to_owned())
}

#[test]
fn four_workers_share_four_hundred_requests() {
    let serve = Serve::start(&["--workers=4", &fixture("whoami.js")], None);
    let port = serve.port;
    let ids: Vec<String> = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| (0..50).filter_map(|_| get(port, "/")).collect::<Vec<_>>()))
            .collect();
        jobs.into_iter()
            .flat_map(|job| job.join().unwrap())
            .collect()
    });
    assert_eq!(ids.len(), 400);
    let distinct: BTreeSet<_> = ids.iter().collect();
    assert!(distinct.len() >= 2, "{distinct:?}");
    assert!(
        distinct
            .iter()
            .all(|id| ["0", "1", "2", "3"].contains(&id.as_str())),
        "{distinct:?}"
    );
}

#[test]
fn a_crashed_worker_is_restarted_and_the_service_continues() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(fixture("crash-once.js"), dir.path().join("crash-once.js")).unwrap();
    let mut serve = Serve::start(
        &[
            "--workers=2",
            "--allow-read=.",
            "--allow-write=.",
            "crash-once.js",
        ],
        Some(dir.path()),
    );
    serve.wait_for("restarting server worker");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ids = BTreeSet::new();
    while ids.len() < 2 {
        assert!(
            Instant::now() < deadline,
            "service did not recover; answered: {ids:?}"
        );
        ids.extend(serve.get("/"));
    }
    assert_eq!(ids, BTreeSet::from(["0".to_owned(), "1".to_owned()]));
}

#[test]
fn the_workers_key_of_velyr_json_sets_the_count() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("velyr.json"), r#"{ "workers": 2 }"#).unwrap();
    std::fs::copy(fixture("whoami.js"), dir.path().join("whoami.js")).unwrap();
    let serve = Serve::start(&["whoami.js"], Some(dir.path()));
    let ids = serve.worker_ids(40);
    assert_eq!(ids, BTreeSet::from(["0".to_owned(), "1".to_owned()]));
}

#[test]
fn zero_workers_is_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .args(["serve", "--workers=0", &fixture("whoami.js")])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("at least 1"));
}

#[test]
fn auto_workers_are_capped_by_the_memory_budget() {
    if std::thread::available_parallelism().map_or(1, usize::from) < 2 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // 256 MiB at 128 MiB per worker is two workers, however many CPUs the host has.
    std::fs::write(
        dir.path().join("velyr.json"),
        r#"{ "workers": "auto", "quotas": { "memoryMb": 256 } }"#,
    )
    .unwrap();
    std::fs::copy(fixture("whoami.js"), dir.path().join("whoami.js")).unwrap();
    let serve = Serve::start(&["whoami.js"], Some(dir.path()));
    let ids = serve.worker_ids(40);
    assert_eq!(ids, BTreeSet::from(["0".to_owned(), "1".to_owned()]));
}

/// The body of `GET /metrics` from the exporter, or `None` while it is not up yet.
fn scrape(port: u16) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .write_all(b"GET /metrics HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
        .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response.starts_with("HTTP/1.1 200").then_some(response)
}

#[test]
fn metrics_sum_the_requests_of_every_worker_and_label_loop_and_heap_per_worker() {
    let metrics_port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let metrics_addr = format!("--metrics-addr=127.0.0.1:{metrics_port}");
    let serve = Serve::start(&["--workers=2", &metrics_addr, &fixture("whoami.js")], None);
    // Keep asking until both workers answered, so both have a finished turn to report.
    let mut seen = BTreeSet::new();
    let mut total = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while seen.len() < 2 || total < 20 {
        assert!(Instant::now() < deadline, "workers answered: {seen:?}");
        seen.extend(serve.get("/"));
        total += 1;
    }
    let text = (0..50)
        .find_map(|_| {
            std::thread::sleep(Duration::from_millis(100));
            scrape(metrics_port).filter(|text| {
                text.contains("velyr_event_loop_lag_seconds{worker=\"0\"}")
                    && text.contains("velyr_event_loop_lag_seconds{worker=\"1\"}")
            })
        })
        .expect("per-worker lag never showed up");
    let line = format!("velyr_http_requests_total{{status=\"2xx\"}} {total}");
    assert!(text.contains(&line), "want {line} in:\n{text}");
    for worker in ["0", "1"] {
        assert!(
            text.contains(&format!("velyr_heap_used_bytes{{worker=\"{worker}\"}}")),
            "{text}"
        );
    }
}

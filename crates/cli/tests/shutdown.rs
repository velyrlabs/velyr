//! `velyr serve` stops on SIGINT/SIGTERM: stop accepting, drain until the deadline, then force.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn fixture(name: &str) -> String {
    format!(
        "{}/../../tests/fixtures/m4/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

struct Server {
    child: Child,
    port: u16,
    lines: BufReader<std::process::ChildStderr>,
}

impl Server {
    /// `velyr serve` on a free loopback port; waits for the "Listening" line.
    fn start(name: &str, workers: &str, deadline_ms: Option<u64>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_velyr"));
        if let Some(ms) = deadline_ms {
            command.env("VELYR_SHUTDOWN_DEADLINE_MS", ms.to_string());
        }
        let mut child = command
            .args([
                "serve",
                "--allow-listen=127.0.0.1:0",
                "--hostname",
                "127.0.0.1",
            ])
            .args(["--port", "0", "--workers", workers])
            .arg(fixture(name))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stderr.take().unwrap());
        let mut line = String::new();
        let port = loop {
            line.clear();
            assert!(lines.read_line(&mut line).unwrap() > 0, "never listened");
            if let Some((_, port)) = line.trim().rsplit_once("127.0.0.1:") {
                break port.trim_end_matches('/').parse().unwrap();
            }
        };
        Self { child, port, lines }
    }

    fn request(&self, path: &str) -> TcpStream {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        stream
    }

    fn wait_for(&mut self, needle: &str) {
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                self.lines.read_line(&mut line).unwrap() > 0,
                "output ended before `{needle}`"
            );
            if line.contains(needle) {
                return;
            }
        }
    }

    fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .args([&format!("-{name}"), &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn exit_code(&mut self) -> Option<i32> {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn sigterm_lets_a_slow_request_finish_across_workers_and_exits_zero() {
    let mut server = Server::start("serve_slow.js", "2", None);
    let mut slow = server.request("/slow");
    server.wait_for("request started");
    server.signal("TERM");
    let mut raw = String::new();
    slow.read_to_string(&mut raw).unwrap();
    assert!(
        raw.starts_with("HTTP/1.1 200") && raw.ends_with("done"),
        "{raw}"
    );
    assert_eq!(server.exit_code(), Some(0));
}

#[test]
fn a_request_that_outlives_the_deadline_forces_exit_one() {
    for workers in ["1", "2"] {
        let mut server = Server::start("serve_hang.js", workers, Some(300));
        let _stuck = server.request("/hang");
        server.wait_for("request started");
        server.signal("TERM");
        assert_eq!(server.exit_code(), Some(1), "workers={workers}");
    }
}

#[test]
fn the_listener_closes_as_soon_as_shutdown_starts() {
    let mut server = Server::start("serve_hang.js", "2", None);
    let _stuck = server.request("/hang");
    server.wait_for("request started");
    server.signal("TERM");
    server.wait_for("shutting down");
    let deadline = Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(("127.0.0.1", server.port)).is_ok() {
        assert!(Instant::now() < deadline, "still accepting connections");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_second_signal_exits_immediately_even_with_workers() {
    let mut server = Server::start("serve_hang.js", "2", None);
    let _stuck = server.request("/hang");
    server.wait_for("request started");
    server.signal("INT");
    server.wait_for("shutting down");
    assert!(
        server.child.try_wait().unwrap().is_none(),
        "exited on the first signal"
    );
    server.signal("INT");
    assert_eq!(server.exit_code(), Some(130));
}

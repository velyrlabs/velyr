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

/// A running `velyr` child; the port comes from the first line `marker` appears on.
struct Server {
    child: Child,
    port: u16,
    lines: BufReader<Box<dyn Read + Send>>,
}

impl Server {
    fn run(name: &str) -> Self {
        Self::spawn(
            &["run", "--allow-listen=127.0.0.1:0", &fixture(name)],
            false,
        )
    }

    fn serve(name: &str) -> Self {
        Self::spawn(
            &[
                "serve",
                "--allow-listen=127.0.0.1:0",
                "--hostname",
                "127.0.0.1",
                "--port",
                "0",
                &fixture(name),
            ],
            true,
        )
    }

    fn serve_with(name: &str, extra: &[&str]) -> Self {
        let mut args = vec!["serve", "--port", "0"];
        args.extend_from_slice(extra);
        let file = fixture(name);
        args.push(&file);
        Self::spawn(&args, true)
    }

    /// `serve` run in `dir`, whose `velyr.json` sets the quotas under test.
    fn serve_with_quotas(name: &str, quotas: &str) -> (Self, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("velyr.json"),
            format!(r#"{{ "quotas": {quotas} }}"#),
        )
        .unwrap();
        std::fs::copy(fixture(name), dir.path().join(name)).unwrap();
        let args = [
            "serve",
            "--port",
            "0",
            "--allow-listen=127.0.0.1:0",
            "--hostname",
            "127.0.0.1",
            name,
        ];
        (Self::spawn_in(&args, true, Some(dir.path())), dir)
    }

    fn spawn(args: &[&str], from_stderr: bool) -> Self {
        Self::spawn_in(args, from_stderr, None)
    }

    fn spawn_in(args: &[&str], from_stderr: bool, dir: Option<&std::path::Path>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_velyr"));
        if let Some(dir) = dir {
            command.current_dir(dir);
        }
        let mut child = command
            .env("VELYR_LEAK_CHECK", "1")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let source: Box<dyn Read + Send> = if from_stderr {
            Box::new(child.stderr.take().unwrap())
        } else {
            Box::new(child.stdout.take().unwrap())
        };
        let mut lines = BufReader::new(source);
        let mut line = String::new();
        let port = loop {
            line.clear();
            assert!(
                lines.read_line(&mut line).unwrap() > 0,
                "server never listened"
            );
            if let Some(port) = line
                .trim()
                .strip_prefix("listening ")
                .or_else(|| line.trim().rsplit_once("127.0.0.1:").map(|(_, port)| port))
            {
                break port.trim_end_matches('/').parse().unwrap();
            }
        };
        Self { child, port, lines }
    }

    fn connect(&self, request: &str) -> TcpStream {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        write!(stream, "{request}").unwrap();
        stream
    }

    /// One `Connection: close` exchange; returns the raw response.
    fn get(&self, path: &str) -> String {
        let mut raw = String::new();
        self.connect(&format!(
            "GET {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n"
        ))
        .read_to_string(&mut raw)
        .unwrap();
        raw
    }

    /// Reads output lines until one contains `needle`.
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

    #[cfg(unix)]
    fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .args([&format!("-{name}"), &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// Remaining lines until the child closes its output.
    fn rest(&mut self) -> String {
        let mut out = String::new();
        self.lines.read_to_string(&mut out).unwrap();
        out
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

fn body(raw: &str) -> &str {
    raw.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

#[test]
fn handler_answers_json_and_failures_are_500() {
    let mut server = Server::run("serve_json.js");
    let ok = server.get("/a?x=1");
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.to_lowercase().contains("application/json"), "{ok}");
    assert_eq!(
        body(&ok),
        r#"{"method":"GET","target":"/a?x=1","body":null,"remote":"127.0.0.1","addr":true}"#
    );
    let post = server.connect(
        "POST /p HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Length: 5\r\n\r\nhello",
    );
    let mut raw = String::new();
    {
        let mut post = post;
        post.read_to_string(&mut raw).unwrap();
    }
    assert!(body(&raw).contains(r#""body":"hello""#), "{raw}");
    assert!(server.get("/boom").starts_with("HTTP/1.1 500"));
    assert!(server.get("/bad").starts_with("HTTP/1.1 500"));
    assert!(server.get("/stop").starts_with("HTTP/1.1 200"));
    assert_eq!(server.exit_code(), Some(0));
    assert!(server.rest().contains("finished"));
}

#[test]
fn streamed_body_reaches_the_client_before_it_ends() {
    let mut server = Server::run("serve_stream.js");
    let mut stream = server.connect("GET /stream HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    // The first chunk arrives while the handler still waits for /release.
    let mut seen = Vec::new();
    let mut buf = [0u8; 256];
    while !String::from_utf8_lossy(&seen).contains("\r\n1\r\na\r\n") {
        let n = stream.read(&mut buf).unwrap();
        assert!(
            n > 0,
            "closed before the first chunk: {}",
            String::from_utf8_lossy(&seen)
        );
        seen.extend_from_slice(&buf[..n]);
    }
    assert!(server.get("/release").ends_with("released"));
    let mut tail = String::new();
    stream.read_to_string(&mut tail).unwrap();
    let all = format!("{}{tail}", String::from_utf8_lossy(&seen));
    let (b, c) = (all.find("\r\n1\r\nb\r\n"), all.find("\r\n1\r\nc\r\n"));
    assert!(b.is_some() && c.is_some() && b < c, "{all}");
    assert!(server.get("/stop").starts_with("HTTP/1.1 200"));
    assert_eq!(server.exit_code(), Some(0));
}

#[test]
fn aborting_the_signal_drains_requests_in_flight_before_finished() {
    let mut server = Server::run("serve_abort.js");
    let mut slow = server.connect("GET /slow HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert!(server.get("/stop").ends_with("stopping"));
    let mut raw = String::new();
    slow.read_to_string(&mut raw).unwrap();
    assert!(
        raw.starts_with("HTTP/1.1 200") && raw.ends_with("slow"),
        "{raw}"
    );
    assert_eq!(server.exit_code(), Some(0));
    assert_eq!(
        server.rest().lines().collect::<Vec<_>>(),
        ["slow done", "finished"]
    );
    assert!(TcpStream::connect(("127.0.0.1", server.port)).is_err());
}

#[test]
fn serve_command_calls_the_default_export_fetch() {
    let server = Server::serve("serve_default.js");
    let raw = server.get("/hi");
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    assert_eq!(body(&raw), "hello /hi");
}

#[test]
fn listening_needs_the_permission() {
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .args(["run", &fixture("serve_json.js")])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .to_lowercase()
            .contains("permission")
    );
}

const LOCAL: [&str; 3] = ["--allow-listen=127.0.0.1:0", "--hostname", "127.0.0.1"];

#[cfg(unix)]
fn drains_request_in_flight_on(signal: &str) {
    let mut server = Server::serve_with("serve_slow.js", &LOCAL);
    let mut slow = server.connect("GET /slow HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    server.wait_for("request started");
    server.signal(signal);
    let mut raw = String::new();
    slow.read_to_string(&mut raw).unwrap();
    assert!(
        raw.starts_with("HTTP/1.1 200") && raw.ends_with("done"),
        "{raw}"
    );
    assert_eq!(server.exit_code(), Some(0));
}

#[cfg(unix)]
#[test]
fn sigint_drains_every_worker_of_a_multicore_server() {
    let mut server = Server::serve_with(
        "serve_slow.js",
        &[
            "--workers",
            "2",
            "--allow-listen=127.0.0.1:0",
            "--hostname",
            "127.0.0.1",
        ],
    );
    let mut slow = server.connect("GET /slow HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    server.wait_for("request started");
    server.signal("INT");
    let mut raw = String::new();
    slow.read_to_string(&mut raw).unwrap();
    assert!(
        raw.starts_with("HTTP/1.1 200") && raw.ends_with("done"),
        "{raw}"
    );
    assert_eq!(server.exit_code(), Some(0));
}

#[cfg(unix)]
#[test]
fn sigint_lets_the_request_in_flight_finish_and_exits_zero() {
    drains_request_in_flight_on("INT");
}

#[cfg(unix)]
#[test]
fn sigterm_lets_the_request_in_flight_finish_and_exits_zero() {
    drains_request_in_flight_on("TERM");
}

#[cfg(unix)]
#[test]
fn a_second_signal_kills_a_stuck_shutdown() {
    let mut server = Server::serve_with("serve_hang.js", &LOCAL);
    let _stuck = server.connect("GET /hang HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    server.wait_for("request started");
    server.signal("INT");
    server.wait_for("shutting down");
    assert!(
        server.child.try_wait().unwrap().is_none(),
        "exited on the first signal"
    );
    server.signal("INT");
    assert_ne!(server.exit_code(), Some(0));
}

#[test]
fn request_signal_aborts_when_the_client_disconnects() {
    let mut server = Server::serve_with("serve_disconnect.js", &LOCAL);
    let client = server.connect("GET /wait HTTP/1.1\r\nHost: t\r\n\r\n");
    server.wait_for("request started");
    drop(client);
    server.wait_for("signal aborted=true");
}

#[test]
fn hostname_can_be_a_name_and_is_checked_as_the_resolved_ip() {
    let server = Server::serve_with(
        "serve_default.js",
        &["--allow-listen=127.0.0.1:0", "--hostname", "localhost"],
    );
    assert_eq!(body(&server.get("/named")), "hello /named");
}

#[test]
fn a_named_hostname_needs_listen_permission_for_its_ip() {
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .args([
            "serve",
            "--port",
            "0",
            "--allow-listen=10.1.2.3:0",
            "--hostname",
            "localhost",
        ])
        .arg(fixture("serve_default.js"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(stderr.contains("permission"), "{stderr}");
}

#[test]
fn a_named_hostname_that_does_not_resolve_is_an_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .args([
            "serve",
            "--port",
            "0",
            "--allow-listen=127.0.0.1:0",
            "--hostname",
            "no-such-host.invalid",
        ])
        .arg(fixture("serve_default.js"))
        .output()
        .unwrap();
    assert!(!output.status.success());
}

fn tls_get(port: u16, ca: Option<&rcgen::Certificate>) -> std::io::Result<String> {
    use std::sync::Arc;
    let mut roots = rustls::RootCertStore::empty();
    if let Some(ca) = ca {
        roots.add(ca.der().clone()).unwrap();
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
    let conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
    let tcp = TcpStream::connect(("127.0.0.1", port))?;
    tcp.set_read_timeout(Some(Duration::from_secs(20)))?;
    let mut tls = rustls::StreamOwned::new(conn, tcp);
    write!(
        tls,
        "GET /tls HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n"
    )?;
    let mut raw = Vec::new();
    // A server that closes without close_notify ends the read with an error after the data.
    let _ = tls.read_to_end(&mut raw);
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

#[test]
fn tls_server_answers_a_client_that_trusts_its_ca() {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("main.js");
    let (pem, key) = (
        serde_json::to_string(&cert.pem()).unwrap(),
        serde_json::to_string(&signing_key.serialize_pem()).unwrap(),
    );
    std::fs::write(
        &script,
        format!(
            r#"Velyr.serve({{ hostname: "127.0.0.1", port: 0, tls: {{ cert: {pem}, key: {key} }},
  onListen: ({{ port }}) => console.log(`listening ${{port}}`) }},
  (request) => new Response(`secure ${{new URL(request.url).protocol}}`));"#
        ),
    )
    .unwrap();
    let server = Server::spawn(
        &[
            "run",
            "--allow-listen=127.0.0.1:0",
            script.to_str().unwrap(),
        ],
        false,
    );
    let raw = tls_get(server.port, Some(&cert)).unwrap();
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    assert_eq!(body(&raw), "secure https:");
    // Without the CA the handshake fails and no response is read.
    assert!(
        !tls_get(server.port, None)
            .unwrap_or_default()
            .contains("200")
    );
}

#[test]
fn one_socket_of_quota_is_enough_for_one_connection_with_one_request() {
    let (server, _dir) = Server::serve_with_quotas("serve_default.js", r#"{ "sockets": 1 }"#);
    let raw = server.get("/hi");
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
}

#[test]
fn n_sockets_of_quota_serve_n_connections_with_requests_in_flight() {
    let (server, _dir) = Server::serve_with_quotas("serve_slow.js", r#"{ "sockets": 3 }"#);
    let open: Vec<TcpStream> = (0..3)
        .map(|_| server.connect("GET / HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n"))
        .collect();
    for mut stream in open {
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    }
}

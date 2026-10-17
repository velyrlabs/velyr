//! `velyr run` with `OTEL_EXPORTER_OTLP_ENDPOINT` exports spans to an OTLP/HTTP collector.
#![cfg(all(feature = "telemetry", feature = "web"))]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

const SCRIPT: &str = r#"
const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, () => new Response("ok"));
const res = await fetch(`http://127.0.0.1:${server.addr.port}/ping?secret=1`);
await res.text();
server.shutdown();
"#;

struct Export {
    path: String,
    content_type: String,
    body: Vec<u8>,
}

/// Accepts OTLP POSTs on a local port, answers 200 and forwards each request.
fn collector() -> (u16, mpsc::Receiver<Export>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = tx.send(read_export(stream));
        }
    });
    (port, rx)
}

fn read_export(mut stream: TcpStream) -> Export {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut chunk).unwrap();
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let header = |name: &str| {
        head.lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix(&format!("{name}:"))
                    .map(|v| v.trim().to_owned())
            })
            .unwrap_or_default()
    };
    let length: usize = header("content-length")
        .parse()
        .expect("content-length body");
    while buf.len() < head_end + length {
        let n = stream.read(&mut chunk).unwrap();
        buf.extend_from_slice(&chunk[..n]);
    }
    stream
        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
        .unwrap();
    Export {
        path: head
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_owned(),
        content_type: header("content-type"),
        body: buf[head_end..head_end + length].to_vec(),
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[test]
fn exports_server_and_client_spans_over_otlp_http() {
    let (port, exports) = collector();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), SCRIPT).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .current_dir(dir.path())
        .env(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            format!("http://127.0.0.1:{port}"),
        )
        .env("OTEL_SERVICE_NAME", "otel-test")
        .args([
            "run",
            "--allow-listen=127.0.0.1:0",
            "--allow-net=127.0.0.1",
            "main.js",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let export = exports
        .recv_timeout(Duration::from_secs(10))
        .expect("an export POST arrived");
    assert_eq!(export.path, "/v1/traces");
    assert_eq!(export.content_type, "application/x-protobuf");
    assert!(contains(&export.body, "otel-test"), "service.name");
    assert!(contains(&export.body, "/ping"), "server span url.path");
    assert!(contains(&export.body, "url.full"), "client span");
    assert!(!contains(&export.body, "secret=1"), "query is redacted");
}

#[test]
fn no_export_without_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), "console.log('hi');").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .current_dir(dir.path())
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .args(["run", "main.js"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
}

const SERVE_SCRIPT: &str = r#"export default {
  fetch: (req) => {
    if (req.url.endsWith("/hang")) return new Promise(() => {});
    if (!req.url.endsWith("/slow")) return new Response("ok");
    return new Response(new ReadableStream({
      async start(controller) {
        controller.enqueue(new TextEncoder().encode("a"));
        await new Promise((resolve) => setTimeout(resolve, 500));
        controller.enqueue(new TextEncoder().encode("b"));
        controller.close();
      },
    }));
  },
};"#;

/// Starts `velyr serve` exporting to `port`; returns the child and the port it listens on.
fn spawn_server(
    dir: &std::path::Path,
    port: u16,
    deadline_ms: &str,
    app: &str,
) -> (std::process::Child, u16) {
    use std::io::BufRead;
    std::fs::write(dir.join("app.js"), app).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .current_dir(dir)
        .env(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            format!("http://127.0.0.1:{port}"),
        )
        .env("VELYR_SHUTDOWN_DEADLINE_MS", deadline_ms)
        .args([
            "serve",
            "--allow-listen=127.0.0.1",
            "--allow-net=127.0.0.1",
            "--port=0",
            "--hostname=127.0.0.1",
            "app.js",
        ])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = std::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let line = lines.next().expect("listening line").unwrap();
    let listen: u16 = line
        .rsplit(':')
        .next()
        .unwrap()
        .trim_end_matches('/')
        .parse()
        .unwrap();
    std::thread::spawn(move || lines.for_each(drop));
    (child, listen)
}

fn interrupt(child: &std::process::Child) {
    Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
}

#[cfg(unix)]
#[test]
fn flushes_spans_when_a_forced_shutdown_exits_early() {
    let (port, exports) = collector();
    let dir = tempfile::tempdir().unwrap();
    let (mut child, listen) = spawn_server(dir.path(), port, "300", SERVE_SCRIPT);

    let mut conn = TcpStream::connect(("127.0.0.1", listen)).unwrap();
    conn.write_all(b"GET /flush-me HTTP/1.1\r\nhost: x\r\n\r\n")
        .unwrap();
    let mut reply = [0u8; 512];
    assert!(conn.read(&mut reply).unwrap() > 0);
    // A request that never answers holds the drain open until the deadline forces the exit.
    let mut hung = TcpStream::connect(("127.0.0.1", listen)).unwrap();
    hung.write_all(b"GET /hang HTTP/1.1\r\nhost: x\r\n\r\n")
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));

    interrupt(&child);
    let status = child.wait().unwrap();
    assert!(!status.success());
    let export = exports
        .recv_timeout(Duration::from_secs(10))
        .expect("spans flushed before exit");
    assert!(contains(&export.body, "/flush-me"));
}

fn varint(buf: &[u8], at: &mut usize) -> u64 {
    let mut value = 0;
    for shift in (0..).step_by(7) {
        let byte = buf[*at];
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
    }
    value
}

/// Fields of one protobuf message: (number, length-delimited bytes or fixed64 as LE bytes).
fn fields(buf: &[u8]) -> Vec<(u64, Vec<u8>)> {
    let (mut at, mut out) = (0, Vec::new());
    while at < buf.len() {
        let tag = varint(buf, &mut at);
        let bytes = match tag & 7 {
            0 => varint(buf, &mut at).to_le_bytes().to_vec(),
            1 => {
                at += 8;
                buf[at - 8..at].to_vec()
            }
            2 => {
                let n = varint(buf, &mut at) as usize;
                at += n;
                buf[at - n..at].to_vec()
            }
            5 => {
                at += 4;
                buf[at - 4..at].to_vec()
            }
            other => panic!("wire type {other}"),
        };
        out.push((tag >> 3, bytes));
    }
    out
}

struct Span {
    name: String,
    kind: u64,
    id: Vec<u8>,
    parent: Vec<u8>,
    nanos: u64,
}

/// Every span in an OTLP traces export.
fn spans(body: &[u8]) -> Vec<Span> {
    let nested = |msg: &[u8], number| -> Vec<Vec<u8>> {
        fields(msg)
            .into_iter()
            .filter(|(n, _)| *n == number)
            .map(|(_, b)| b)
            .collect()
    };
    let field = |span: &[u8], number| {
        fields(span)
            .into_iter()
            .find(|(n, _)| *n == number)
            .map(|(_, b)| b)
            .unwrap_or_default()
    };
    let fixed = |span: &[u8], number| u64::from_le_bytes(field(span, number).try_into().unwrap());
    nested(body, 1)
        .iter()
        .flat_map(|resource| nested(resource, 2))
        .flat_map(|scope| nested(&scope, 2))
        .map(|span| Span {
            name: String::from_utf8(field(&span, 5)).unwrap(),
            kind: u64::from_le_bytes(field(&span, 6).try_into().unwrap_or([0; 8])),
            id: field(&span, 2),
            parent: field(&span, 4),
            nanos: fixed(&span, 8) - fixed(&span, 7),
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn server_span_covers_a_streamed_body() {
    let (port, exports) = collector();
    let dir = tempfile::tempdir().unwrap();
    let (mut child, listen) = spawn_server(dir.path(), port, "5000", SERVE_SCRIPT);

    let mut conn = TcpStream::connect(("127.0.0.1", listen)).unwrap();
    conn.write_all(b"GET /slow HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
        .unwrap();
    let mut reply = Vec::new();
    conn.read_to_end(&mut reply).unwrap();
    interrupt(&child);
    child.wait().unwrap();

    let export = exports
        .recv_timeout(Duration::from_secs(10))
        .expect("an export");
    let server = spans(&export.body)
        .into_iter()
        .find(|span| span.name == "GET")
        .expect("the server span");
    assert!(
        server.nanos >= 400_000_000,
        "span ended before the body did: {} ns",
        server.nanos
    );
}

#[test]
fn script_spans_are_exported_and_failures_rethrown() {
    let (port, exports) = collector();
    let dir = tempfile::tempdir().unwrap();
    let script = r#"
const value = await Velyr.telemetry.span("load-orders", async () => 7);
if (value !== 7) throw new Error("value lost");
try { Velyr.telemetry.span("explode", () => { throw new Error("kaput"); }); } catch (e) { if (e.message !== "kaput") throw e; }
"#;
    std::fs::write(dir.path().join("main.js"), script).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .current_dir(dir.path())
        .env(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            format!("http://127.0.0.1:{port}"),
        )
        .args(["run", "main.js"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let export = exports
        .recv_timeout(Duration::from_secs(10))
        .expect("an export");
    assert!(contains(&export.body, "load-orders"));
    assert!(contains(&export.body, "explode"));
    assert!(contains(&export.body, "kaput"));
}

#[test]
fn script_span_runs_the_function_without_an_exporter() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("main.js"),
        "console.log(await Velyr.telemetry.span('x', async () => 'ran'));",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .current_dir(dir.path())
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .args(["run", "main.js"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ran");
}

#[test]
fn exports_runtime_metrics_over_otlp_http_on_exit() {
    let (port, exports) = collector();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), "console.log('hi');").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .current_dir(dir.path())
        .env(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            format!("http://127.0.0.1:{port}"),
        )
        .env("OTEL_SERVICE_NAME", "otel-metrics")
        .args(["run", "main.js"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let export = std::iter::from_fn(|| exports.recv_timeout(Duration::from_secs(10)).ok())
        .find(|export| export.path == "/v1/metrics")
        .expect("a metrics export arrived");
    assert_eq!(export.content_type, "application/x-protobuf");
    assert!(contains(&export.body, "otel-metrics"), "service.name");
    assert!(contains(
        &export.body,
        "velyr_process_resident_memory_bytes"
    ));
    assert!(contains(&export.body, "velyr_http_requests_total"));
}

#[test]
fn https_endpoint_starts_a_tls_handshake() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, first_bytes) = mpsc::channel();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut byte = [0u8; 1];
            if stream.read_exact(&mut byte).is_ok() {
                let _ = tx.send(byte[0]);
            }
        }
    });
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), "console.log('hi');").unwrap();
    Command::new(env!("CARGO_BIN_EXE_velyr"))
        .current_dir(dir.path())
        .env(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            format!("https://127.0.0.1:{port}"),
        )
        .args(["run", "main.js"])
        .output()
        .unwrap();
    // 0x16 is the TLS handshake record type; plain HTTP would start with 'P' (POST).
    assert_eq!(
        first_bytes
            .recv_timeout(Duration::from_secs(10))
            .expect("a connection"),
        0x16
    );
}

#[cfg(unix)]
#[test]
fn fetch_and_script_spans_inside_a_handler_are_children_of_the_server_span() {
    let (port, exports) = collector();
    let dir = tempfile::tempdir().unwrap();
    // The handler calls itself once: /outer -> span "inner" -> fetch /leaf.
    let app = r#"export default {
  fetch: async (req) => {
    const url = new URL(req.url);
    if (url.pathname !== "/outer") return new Response("leaf");
    await Velyr.telemetry.span("inner", async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
      await (await fetch(new URL("/leaf", url))).text();
    });
    return new Response("ok");
  },
};"#;
    let (mut child, listen) = spawn_server(dir.path(), port, "5000", app);
    let mut conn = TcpStream::connect(("127.0.0.1", listen)).unwrap();
    conn.write_all(
        format!("GET /outer HTTP/1.1\r\nhost: 127.0.0.1:{listen}\r\nconnection: close\r\n\r\n")
            .as_bytes(),
    )
    .unwrap();
    conn.read_to_end(&mut Vec::new()).unwrap();
    interrupt(&child);
    child.wait().unwrap();

    let export = exports
        .recv_timeout(Duration::from_secs(10))
        .expect("an export");
    let mut all = spans(&export.body);
    while let Ok(more) = exports.recv_timeout(Duration::from_secs(1)) {
        if more.path == "/v1/traces" {
            all.extend(spans(&more.body));
        }
    }
    let by_name = |name: &str, kind: u64| {
        all.iter()
            .find(|s| s.name == name && s.kind == kind)
            .unwrap_or_else(|| panic!("{name} span"))
    };
    let (inner, client) = (by_name("inner", 1), by_name("GET", 3));
    let server = all
        .iter()
        .find(|s| s.kind == 2 && s.id == inner.parent)
        .expect("inner is a child of a server span");
    assert!(
        server.parent.is_empty(),
        "the outer server span is the root"
    );
    assert_eq!(
        client.parent, inner.id,
        "fetch is a child of the script span"
    );
    assert!(
        all.iter().any(|s| s.kind == 2 && s.parent == client.id),
        "leaf server span continues the fetch"
    );
}

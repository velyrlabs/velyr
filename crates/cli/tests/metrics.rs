//! `Velyr.metrics()` and the `--metrics-addr` Prometheus exporter.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Kills the child on drop, so a failed assertion cannot leave a spinning process behind.
struct Spinner(Child);

impl Drop for Spinner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

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

fn value(text: &str, name: &str) -> f64 {
    text.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("{name} missing in:\n{text}"))
        .parse()
        .unwrap()
}

#[test]
fn metrics_endpoint_answers_while_javascript_is_saturated() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("spin.js");
    std::fs::write(&script, "while (true) {}").unwrap();
    let port = free_port();
    let _spinner = Spinner(
        Command::new(env!("CARGO_BIN_EXE_velyr"))
            .arg(format!("--metrics-addr=127.0.0.1:{port}"))
            .arg("run")
            .arg(&script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let text = loop {
        // Wait until the spin loop has held the loop for a while: the lag is how long.
        if let Some(text) = scrape(port)
            && value(&text, "velyr_event_loop_lag_seconds") >= 0.2
        {
            break text;
        }
        assert!(
            Instant::now() < deadline,
            "no scrape with a busy loop in time"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(text.contains("content-type: text/plain"), "{text}");
    assert!(value(&text, "velyr_process_resident_memory_bytes") > 0.0);
}

#[test]
fn a_permission_denial_increments_the_counter() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("denied.js");
    std::fs::write(
        &script,
        "const before = Velyr.metrics().permissionDenied;
         try { await fetch('http://127.0.0.1:9/'); } catch {}
         console.log(before, Velyr.metrics().permissionDenied);",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .arg("run")
        .arg(&script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0 1");
}

#[test]
fn served_requests_show_up_in_the_http_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("serve.js");
    std::fs::write(
        &script,
        "const server = Velyr.serve({ hostname: '127.0.0.1', port: 0 }, () => new Response('ok'));
         await (await fetch(`http://127.0.0.1:${server.addr.port}/`)).text();
         await server.shutdown();
         const http = Velyr.metrics().http;
         console.log(http.requests, http.byStatus['2xx']);",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .args(["run", "--allow-listen=127.0.0.1:0", "--allow-net=127.0.0.1"])
        .arg(&script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1 1");
}

#[cfg(feature = "engine-native")]
fn get(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response.starts_with("HTTP/1.1 200").then_some(response)
}

#[cfg(feature = "engine-native")]
#[test]
fn serve_on_the_native_engine_reports_its_heap() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.js");
    std::fs::write(
        &script,
        "export default { fetch() { return new Response('native'); } };",
    )
    .unwrap();
    let (port, metrics) = (free_port(), free_port());
    let _server = Spinner(
        Command::new(env!("CARGO_BIN_EXE_velyr"))
            .arg(format!("--metrics-addr=127.0.0.1:{metrics}"))
            .args(["serve", "--engine", "native", "--hostname", "127.0.0.1"])
            .arg(format!("--allow-listen=127.0.0.1:{port}"))
            .arg(format!("--port={port}"))
            .arg(&script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let body = loop {
        if let Some(body) = get(port, "/") {
            break body;
        }
        assert!(
            Instant::now() < deadline,
            "the native server never answered"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(body.ends_with("native"), "{body}");
    let text = scrape(metrics).expect("metrics scrape");
    assert_eq!(
        value(&text, "velyr_http_requests_total{status=\"2xx\"}"),
        1.0
    );
    assert!(value(&text, "velyr_heap_used_bytes") > 0.0, "{text}");
    assert!(value(&text, "velyr_heap_external_bytes") > 0.0, "{text}");
    assert!(value(&text, "velyr_heap_cells") > 0.0, "{text}");
    assert!(value(&text, "velyr_gc_collections_total") >= 0.0, "{text}");
    assert!(
        value(&text, "velyr_gc_pause_seconds_total") >= 0.0,
        "{text}"
    );
}

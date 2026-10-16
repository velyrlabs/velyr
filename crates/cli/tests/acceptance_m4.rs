//! M4 acceptance: a Web-standard server (`Velyr.serve` through `velyr serve`) and a `fetch`
//! client, each under scoped grants. Run it against the web profile too:
//! `--no-default-features --features profile-web`.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use assert_cmd::assert::Assert;

fn fixtures() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/m4-acceptance")
}

/// `velyr serve server.js` run from the fixture directory, so the module is inside the project.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn start(grants: &[&str]) -> Self {
        let mut child = Self::command(grants)
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, lines) = mpsc::channel();
        let pipe = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        let port = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = lines.recv_timeout(left).expect("server never listened");
            if let Some((_, port)) = line.split_once("Listening on http://127.0.0.1:") {
                break port.trim_end_matches('/').parse().unwrap();
            }
        };
        Self { child, port }
    }

    fn command(grants: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_velyr"));
        command
            .current_dir(fixtures())
            .env("VELYR_LEAK_CHECK", "1")
            .args(["serve", "--hostname", "127.0.0.1", "--port", "0"])
            .args(grants)
            .arg("server.js")
            .stdout(Stdio::null());
        command
    }
}

/// Runs `script` as `velyr run <grants>` in a scratch directory.
fn client(script: &str, grants: &[&str]) -> Assert {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("client.js"), script).unwrap();
    assert_cmd::Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir.path())
        .env("VELYR_LEAK_CHECK", "1")
        .timeout(Duration::from_secs(30))
        .arg("run")
        .args(grants)
        .arg("client.js")
        .assert()
}

const LISTEN: &str = "--allow-listen=127.0.0.1:0";

#[test]
fn a_scoped_server_and_a_scoped_client_exchange_plaintext_json_and_a_stream() {
    let server = Server::start(&[LISTEN]);
    let base = format!("http://127.0.0.1:{}", server.port);
    client(
        &format!(
            "const text = await fetch('{base}/plaintext');
             console.log(text.status, await text.text());
             console.log(JSON.stringify(await (await fetch('{base}/json')).json()));
             const chunks = [];
             for await (const chunk of (await fetch('{base}/stream')).body) chunks.push(chunk.length);
             console.log(chunks.length >= 1 && chunks.length <= 16, chunks.reduce((a, b) => a + b, 0));"
        ),
        &[&format!("--allow-net=127.0.0.1:{}", server.port)],
    )
    .success()
    .stdout("200 Hello, World!\n{\"message\":\"Hello, World!\"}\ntrue 1024\n");
}

#[test]
fn without_a_listen_grant_the_server_does_not_start() {
    let output = Server::command(&[])
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("PERMISSION_DENIED"), "{stderr}");
}

#[test]
fn without_a_net_grant_the_client_does_not_connect() {
    let server = Server::start(&[LISTEN]);
    client(
        &format!(
            "try {{ await fetch('http://127.0.0.1:{}/plaintext'); }} catch (e) {{ console.log(e.code); }}",
            server.port
        ),
        &[],
    )
    .success()
    .stdout("VELYR_PERMISSION_DENIED\n");
}

#[test]
fn a_grant_for_the_origin_does_not_follow_a_redirect_to_another_host() {
    let server = Server::start(&[LISTEN]);
    let port = server.port;
    let script = format!(
        "try {{ const r = await fetch('http://127.0.0.1:{port}/redirect'); console.log(r.status, await r.text()); }}
         catch (e) {{ console.log(e.code); }}"
    );
    client(&script, &[&format!("--allow-net=127.0.0.1:{port}")])
        .success()
        .stdout("VELYR_PERMISSION_DENIED\n");
    // Control: with the second host granted the same redirect is followed.
    client(
        &script,
        &[&format!("--allow-net=127.0.0.1:{port},localhost:{port}")],
    )
    .success()
    .stdout("200 Hello, World!\n");
}

#[cfg(not(feature = "node"))]
#[test]
fn the_web_profile_does_not_link_node_compat() {
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .current_dir(fixtures())
        .args(["run", "node_fs.js"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Node compatibility, which is not linked"),
        "{stderr}"
    );
}

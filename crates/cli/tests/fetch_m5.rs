use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use assert_cmd::Command;

/// Answers every connection with `reply(path)` after reading the request head; one thread each.
fn serve(reply: impl Fn(&str) -> String + Send + Sync + 'static) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let reply = std::sync::Arc::new(reply);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let reply = reply.clone();
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head);
                let path = head.split(' ').nth(1).unwrap_or("/").to_owned();
                let _ = stream.write_all(reply(&path).as_bytes());
            });
        }
    });
    port
}

fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn run(script: &str, allow_net: &str) -> assert_cmd::assert::Assert {
    run_with_env(script, allow_net, &[])
}

fn run_with_env(
    script: &str,
    allow_net: &str,
    env: &[(&str, &std::path::Path)],
) -> assert_cmd::assert::Assert {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.js");
    std::fs::write(&path, script).unwrap();
    let mut command = Command::cargo_bin("velyr").unwrap();
    for (name, value) in env {
        command.env(name, value);
    }
    command
        .env("VELYR_LEAK_CHECK", "1")
        .timeout(Duration::from_secs(30))
        .arg("run")
        .arg(allow_net)
        .arg(&path)
        .assert()
}

#[test]
fn fetch_reads_json_from_a_local_server_with_a_grant() {
    let port = serve(|_| json_response(r#"{"a":1}"#));
    run(
        &format!(
            "const res = await fetch('http://127.0.0.1:{port}/');
             console.log(res.status, res.headers.get('content-type'), JSON.stringify(await res.json()));
             console.log(await Response.json({{ a: 1 }}).text());"
        ),
        "--allow-net=127.0.0.1",
    )
    .success()
    .stdout("200 application/json {\"a\":1}\n{\"a\":1}\n");
}

#[test]
fn allow_all_fetches_a_loopback_literal_but_bare_allow_net_does_not() {
    let port = serve(|_| json_response("{}"));
    let script = format!(
        "try {{ console.log((await fetch('http://127.0.0.1:{port}/')).status); }} catch (e) {{ console.log(e.code); }}"
    );
    run(&script, "-A").success().stdout("200\n");
    run(&script, "--allow-net")
        .success()
        .stdout("VELYR_PERMISSION_DENIED\n");
}

#[test]
fn fetch_without_a_grant_is_denied() {
    let port = serve(|_| json_response("{}"));
    run(
        &format!(
            "try {{ await fetch('http://127.0.0.1:{port}/'); }} catch (e) {{ console.log(e.code); }}"
        ),
        "--allow-net=example.com",
    )
    .success()
    .stdout("VELYR_PERMISSION_DENIED\n");
}

#[test]
fn redirect_to_a_host_without_a_grant_is_denied() {
    let port = serve(|_| {
        "HTTP/1.1 302 Found\r\nconnection: close\r\ncontent-length: 0\r\nlocation: http://localhost:9/\r\n\r\n".into()
    });
    run(
        &format!(
            "try {{ await fetch('http://127.0.0.1:{port}/'); }} catch (e) {{ console.log(e.code); }}"
        ),
        "--allow-net=127.0.0.1",
    )
    .success()
    .stdout("VELYR_PERMISSION_DENIED\n");
}

#[test]
fn an_unread_response_body_is_not_reported_as_a_leak() {
    let port = serve(|_| json_response("{}"));
    run(
        &format!("const res = await fetch('http://127.0.0.1:{port}/'); console.log(res.status);"),
        "--allow-net=127.0.0.1",
    )
    .success()
    .stdout("200\n");
}

/// Replies to every connection with `reply(head)`, where `head` is the whole request head.
fn serve_head(reply: impl Fn(&str) -> String + Send + Sync + 'static) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let reply = std::sync::Arc::new(reply);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let reply = reply.clone();
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let _ = stream.write_all(reply(&String::from_utf8_lossy(&head)).as_bytes());
            });
        }
    });
    port
}

#[test]
fn a_cross_origin_redirect_drops_authorization_cookie_and_proxy_authorization() {
    let target = serve_head(|head| json_response(&format!("{:?}", head.to_ascii_lowercase())));
    // Same host, another port: a different origin.
    let origin = serve_head(move |_| {
        format!(
            "HTTP/1.1 302 Found\r\nconnection: close\r\ncontent-length: 0\r\nlocation: http://127.0.0.1:{target}/\r\n\r\n"
        )
    });
    let assert = run(
        &format!(
            "const res = await fetch('http://127.0.0.1:{origin}/', {{ headers: {{
               authorization: 'Bearer secret', cookie: 'a=1', 'proxy-authorization': 'Basic x', 'x-keep': '1' }} }});
             console.log(await res.text());"
        ),
        "--allow-net=127.0.0.1",
    )
    .success();
    let seen = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(seen.contains("x-keep"), "{seen}");
    for stripped in ["authorization", "cookie", "secret"] {
        assert!(!seen.contains(stripped), "{seen}");
    }
}

/// A one-shot HTTPS server (HTTP/1.1) with a fresh self-signed certificate for 127.0.0.1.
fn https_server() -> (u16, tempfile::TempDir, std::path::PathBuf) {
    use std::sync::Arc;

    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], key)
        .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for tcp in listener.incoming() {
            let Ok(tcp) = tcp else { return };
            let config = config.clone();
            std::thread::spawn(move || {
                let Ok(connection) = rustls::ServerConnection::new(config) else {
                    return;
                };
                let mut tls = rustls::StreamOwned::new(connection, tcp);
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && tls.read(&mut byte).unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let _ = tls.write_all(json_response(r#"{"secure":true}"#).as_bytes());
                tls.conn.send_close_notify();
                let _ = tls.flush();
            });
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, cert.pem()).unwrap();
    (port, dir, ca)
}

fn https_script(port: u16) -> String {
    format!(
        "try {{ const res = await fetch('https://127.0.0.1:{port}/'); console.log(await res.text()); }}
         catch (e) {{ console.log('failed', e.name, e.cause?.message ?? e.message); }}"
    )
}

#[test]
fn https_fetch_trusts_the_extra_ca_and_still_verifies_without_it() {
    let (port, _dir, ca) = https_server();
    run_with_env(
        &https_script(port),
        "--allow-net=127.0.0.1",
        &[("VELYR_EXTRA_CA_CERTS", &ca)],
    )
    .success()
    .stdout("{\"secure\":true}\n");

    let untrusted = run(&https_script(port), "--allow-net=127.0.0.1").success();
    let out = String::from_utf8_lossy(&untrusted.get_output().stdout).into_owned();
    assert!(out.starts_with("failed TypeError"), "{out}");
}

#[test]
fn an_unreadable_or_certificate_less_extra_ca_file_is_an_error() {
    let port = serve(|_| json_response("{}"));
    let dir = tempfile::tempdir().unwrap();
    let garbage = dir.path().join("garbage.pem");
    std::fs::write(&garbage, "not a certificate").unwrap();
    for file in [garbage, dir.path().join("missing.pem")] {
        let assert = run_with_env(
            &format!(
                "try {{ await fetch('http://127.0.0.1:{port}/'); console.log('fetched'); }}
                 catch (e) {{ console.log(e.cause?.message ?? e.message); }}"
            ),
            "--allow-net=127.0.0.1",
            &[("VELYR_EXTRA_CA_CERTS", &file)],
        )
        .success();
        let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        assert!(out.contains("VELYR_EXTRA_CA_CERTS"), "{out}");
    }
}

#[test]
fn an_upload_outlives_the_response_without_leaking_a_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut seen = Vec::new();
        let mut byte = [0u8; 1];
        let mut answered = false;
        while !seen.ends_with(b"\r\n0\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
            seen.push(byte[0]);
            if !answered && seen.ends_with(b"\r\n\r\n") {
                answered = true;
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok");
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&seen).into_owned());
    });
    let script = format!(
        "let release;
         const gate = new Promise((resolve) => {{ release = resolve; }});
         let step = 0;
         const body = new ReadableStream({{ async pull(c) {{
           if (step++ === 0) {{ c.enqueue(new TextEncoder().encode('first')); return; }}
           await gate;
           c.enqueue(new TextEncoder().encode('second'));
           c.close();
         }} }});
         const res = await fetch('http://127.0.0.1:{port}/', {{ method: 'PUT', body, duplex: 'half' }});
         console.log(await res.text());
         release();"
    );
    run(&script, "--allow-net=127.0.0.1")
        .success()
        .stdout("ok\n");
    let request = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        request.contains("first") && request.contains("second"),
        "{request}"
    );
}

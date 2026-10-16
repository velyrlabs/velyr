use std::time::Duration;

use assert_cmd::Command;

fn script(cert_pem: &str, key_pem: &str) -> String {
    let (cert, key) = (
        serde_json::to_string(cert_pem).unwrap(),
        serde_json::to_string(key_pem).unwrap(),
    );
    format!(
        r#"
const cert = {cert};
const listener = await Velyr.listenTls({{ hostname: "127.0.0.1", port: 0, cert, key: {key} }});
const server = (async () => {{
  for await (const conn of listener) {{
    await conn.readable.pipeTo(conn.writable);
    conn.close();
    break;
  }}
}})();
const conn = await Velyr.connectTls({{
  hostname: "127.0.0.1",
  port: listener.addr.port,
  caCerts: [cert],
}});
const writer = conn.writable.getWriter();
await writer.write(new TextEncoder().encode("hello tls"));
await writer.close();
let text = "";
for await (const chunk of conn.readable.pipeThrough(new TextDecoderStream())) text += chunk;
console.log(text);
conn.close();
await server;
listener.close();
"#
    )
}

#[test]
fn tls_echo_over_loopback_with_a_self_signed_ca() {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.js");
    std::fs::write(&path, script(&cert.pem(), &signing_key.serialize_pem())).unwrap();
    Command::cargo_bin("velyr")
        .unwrap()
        .env("VELYR_LEAK_CHECK", "1")
        .timeout(Duration::from_secs(30))
        .arg("run")
        .args(["--allow-net=127.0.0.1", "--allow-listen=127.0.0.1"])
        .arg(&path)
        .assert()
        .success()
        .stdout("hello tls\n");
}

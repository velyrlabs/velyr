use assert_cmd::Command;

const ECHO: &str = r#"
const listener = await Velyr.listen({ hostname: "127.0.0.1", port: 0 });
const server = (async () => {
  for await (const conn of listener) {
    await conn.readable.pipeTo(conn.writable);
    conn.close();
    break;
  }
})();
const conn = await Velyr.connect({ hostname: "127.0.0.1", port: listener.addr.port });
const writer = conn.writable.getWriter();
await writer.write(new TextEncoder().encode("hello net"));
await writer.close();
let text = "";
for await (const chunk of conn.readable.pipeThrough(new TextDecoderStream())) text += chunk;
console.log(text);
conn.close();
await server;
listener.close();
"#;

fn run_script(script: &str, flags: &[&str]) -> assert_cmd::assert::Assert {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.js");
    std::fs::write(&path, script).unwrap();
    Command::cargo_bin("velyr")
        .unwrap()
        .env("VELYR_LEAK_CHECK", "1")
        .arg("run")
        .args(flags)
        .arg(&path)
        .assert()
}

#[test]
fn echo_server_and_client_over_loopback() {
    run_script(ECHO, &["--allow-listen=127.0.0.1", "--allow-net=127.0.0.1"])
        .success()
        .stdout("hello net\n");
}

#[test]
fn listen_without_allow_listen_is_denied() {
    run_script(
        r#"try { await Velyr.listen({ hostname: "127.0.0.1", port: 0 }); }
catch (e) { console.log(e.code); }"#,
        &[],
    )
    .success()
    .stdout("VELYR_PERMISSION_DENIED\n");
}

//! `velyr run --inspect*`: the CDP server is opt-in, serves `/json/*`, and a debugger client can
//! stop a TypeScript module at a breakpoint.
#![cfg(feature = "inspector")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;

const TIMEOUT: Duration = Duration::from_secs(20);

const SLEEPER_JS: &str = r#"
console.log("ready");
await new Promise((resolve) => setTimeout(resolve, 1500));
"#;

const MAIN_TS: &str = r#"interface Point {
  x: number;
  y: number;
}
type Id = string;
const p: Point = { x: 1, y: 2 };
const id: Id = "a";
function mark(n: number): number {
  return n + 41;
}
console.log("value", mark(p.x), id);
"#;

fn velyr(dir: &Path, args: &[&str]) -> Child {
    let home = dir.join("home");
    Command::new(env!("CARGO_BIN_EXE_velyr"))
        .current_dir(dir)
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("LOCALAPPDATA", home.join("cache"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Forwards each line of `stream` (stdout or stderr of the child) to a channel.
fn lines(stream: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    rx
}

fn wait_for_line(lines: &mpsc::Receiver<String>, needle: &str) -> String {
    loop {
        let line = lines
            .recv_timeout(TIMEOUT)
            .unwrap_or_else(|_| panic!("no line containing {needle:?}"));
        if line.contains(needle) {
            return line;
        }
    }
}

/// The `ws://` URL from the "Debugger listening on" line.
fn debugger_url(stderr: &mpsc::Receiver<String>) -> String {
    let line = wait_for_line(stderr, "Debugger listening on ");
    line.rsplit(' ').next().unwrap().to_owned()
}

fn listening_sockets(pid: u32) -> Option<String> {
    let out = Command::new("lsof")
        .args(["-nP", "-a", "-p", &pid.to_string(), "-iTCP", "-sTCP:LISTEN"])
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn http_get(authority: &str, path: &str) -> (String, String) {
    let mut stream = TcpStream::connect(authority).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.to_owned(), body.to_owned())
}

#[test]
fn no_socket_is_opened_without_an_inspect_flag_even_with_allow_all() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), SLEEPER_JS).unwrap();
    let mut child = velyr(dir.path(), &["run", "-A", "main.js"]);
    let stdout = lines(child.stdout.take().unwrap());
    let stderr = lines(child.stderr.take().unwrap());
    wait_for_line(&stdout, "ready");

    match listening_sockets(child.id()) {
        Some(sockets) => assert!(sockets.trim().is_empty(), "listening: {sockets}"),
        None => eprintln!("lsof is not available; only the stderr banner is checked"),
    }
    assert!(child.wait().unwrap().success());
    let errors: Vec<String> = stderr.try_iter().collect();
    assert!(
        !errors.iter().any(|l| l.contains("Debugger listening")),
        "{errors:?}"
    );
}

#[test]
fn serves_json_version_and_list_and_rejects_a_wrong_token() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), SLEEPER_JS).unwrap();
    let mut child = velyr(dir.path(), &["run", "--inspect=127.0.0.1:0", "main.js"]);
    let stderr = lines(child.stderr.take().unwrap());
    let url = debugger_url(&stderr);
    let authority = url
        .strip_prefix("ws://")
        .and_then(|rest| rest.split_once('/'))
        .map(|(authority, _)| authority.to_owned())
        .unwrap();
    let token = url.rsplit('/').next().unwrap();
    assert!(token.len() >= 32, "token too short: {url}");

    let (head, body) = http_get(&authority, "/json/version");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let version: Value = serde_json::from_str(&body).unwrap();
    assert!(
        version["Browser"].as_str().unwrap().starts_with("Velyr/"),
        "{body}"
    );
    assert_eq!(version["Protocol-Version"], "1.3");

    let (_, body) = http_get(&authority, "/json/list");
    let targets: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(targets[0]["webSocketDebuggerUrl"], url.as_str(), "{body}");

    let wrong = format!("ws://{authority}/not-the-token");
    assert!(tungstenite::connect(wrong.as_str()).is_err());
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn inspect_is_refused_when_denied_or_on_the_native_engine() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), SLEEPER_JS).unwrap();
    for args in [
        &["run", "--inspect", "--deny-inspect", "main.js"][..],
        &["run", "--inspect", "--engine", "native", "main.js"][..],
    ] {
        let out = velyr(dir.path(), args).wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}: {stderr}");
        assert!(stderr.contains("inspector"), "{args:?}: {stderr}");
        assert!(!stderr.contains("Debugger listening"), "{args:?}: {stderr}");
    }
}

/// A CDP client over the WebSocket; `notifications` keeps every event seen so far.
struct Cdp {
    socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    notifications: Vec<Value>,
}

impl Cdp {
    fn connect(url: &str) -> Self {
        let (socket, _) = tungstenite::connect(url).unwrap();
        if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        }
        Self {
            socket,
            next_id: 0,
            notifications: Vec::new(),
        }
    }

    fn read(&mut self) -> Value {
        loop {
            let message = self.socket.read().expect("CDP message");
            if let tungstenite::Message::Text(text) = message {
                return serde_json::from_str(text.as_str()).unwrap();
            }
        }
    }

    fn send(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let request = json!({ "id": self.next_id, "method": method, "params": params });
        self.socket
            .send(tungstenite::Message::text(request.to_string()))
            .unwrap();
        self.next_id
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        loop {
            let message = self.read();
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{method}: {message}");
                return message["result"].clone();
            }
            self.notifications.push(message);
        }
    }

    fn wait_for(&mut self, method: &str) -> Value {
        if let Some(at) = self
            .notifications
            .iter()
            .position(|m| m["method"] == method)
        {
            return self.notifications.remove(at)["params"].clone();
        }
        loop {
            let message = self.read();
            if message["method"] == method {
                return message["params"].clone();
            }
            self.notifications.push(message);
        }
    }

    fn parsed_script(&self, suffix: &str) -> Value {
        self.notifications
            .iter()
            .filter(|m| m["method"] == "Debugger.scriptParsed")
            .map(|m| &m["params"])
            .find(|p| p["url"].as_str().is_some_and(|url| url.ends_with(suffix)))
            .cloned()
            .unwrap_or_else(|| panic!("no scriptParsed for {suffix}"))
    }
}

fn decode_inline_map(url: &str) -> Value {
    let data = url
        .strip_prefix("data:application/json;base64,")
        .unwrap_or_else(|| panic!("not an inline map: {url}"));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn a_breakpoint_in_a_typescript_module_pauses_execution() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.ts"), MAIN_TS).unwrap();
    let mut child = velyr(dir.path(), &["run", "--inspect-brk=127.0.0.1:0", "main.ts"]);
    let stdout = lines(child.stdout.take().unwrap());
    let stderr = lines(child.stderr.take().unwrap());
    let mut cdp = Cdp::connect(&debugger_url(&stderr));

    cdp.call("Debugger.enable", json!({}));
    cdp.call("Runtime.runIfWaitingForDebugger", json!({}));
    let start = cdp.wait_for("Debugger.paused");
    let script = cdp.parsed_script("/main.ts");
    let top = &start["callFrames"][0]["location"];
    assert_eq!(top["scriptId"], script["scriptId"], "{start}");

    let map = decode_inline_map(script["sourceMapURL"].as_str().unwrap());
    assert!(map["sources"].to_string().contains("main.ts"), "{map}");

    let source = cdp.call(
        "Debugger.getScriptSource",
        json!({ "scriptId": script["scriptId"] }),
    );
    let generated = source["scriptSource"].as_str().unwrap();
    let line = generated
        .lines()
        .position(|l| l.contains("n + 41"))
        .unwrap_or_else(|| panic!("{generated}"));
    let breakpoint = cdp.call(
        "Debugger.setBreakpointByUrl",
        json!({ "url": script["url"], "lineNumber": line }),
    );
    assert!(
        !breakpoint["locations"].as_array().unwrap().is_empty(),
        "{breakpoint}"
    );
    assert!(stdout.try_recv().is_err(), "ran before the breakpoint");

    cdp.call("Debugger.resume", json!({}));
    let hit = cdp.wait_for("Debugger.paused");
    assert_eq!(
        hit["hitBreakpoints"][0], breakpoint["breakpointId"],
        "{hit}"
    );
    assert_eq!(
        hit["callFrames"][0]["location"]["lineNumber"], line,
        "{hit}"
    );
    assert_eq!(hit["callFrames"][0]["functionName"], "mark", "{hit}");
    assert!(stdout.try_recv().is_err(), "ran past the breakpoint");

    // The program ends right after; its process may exit before the response is written.
    cdp.send("Debugger.resume", json!({}));
    assert_eq!(wait_for_line(&stdout, "value"), "value 42 a");
    assert!(child.wait().unwrap().success());
}

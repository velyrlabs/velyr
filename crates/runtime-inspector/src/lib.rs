//! Chrome DevTools Protocol transport for `velyr run --inspect*`: `/json/version`, `/json/list`
//! and one WebSocket session at `ws://ADDR/<token>`. Engine-agnostic: CDP messages go in and out
//! as text through [`Event`]s; the engine adapter dispatches them.
//!
//! The token is 128 random bits, so only a client that read the URL (stderr or `/json/list` on
//! the bound address) can attach. Requests whose `Host` is not an IP literal or `localhost` are
//! refused, which stops DNS-rebinding pages from reading `/json/list`.

use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use tungstenite::handshake::derive_accept_key;
use tungstenite::protocol::Role;
use tungstenite::{Message, WebSocket};

/// Outgoing messages are flushed at least this often while a session is idle.
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_HEAD_BYTES: usize = 16 * 1024;
const PROTOCOL_VERSION: &str = "1.3";

/// What the server reports to the engine side.
#[derive(Debug)]
pub enum Event {
    /// A client attached; CDP responses and notifications for it go to this sender.
    Connected(Sender<String>),
    /// One CDP message from the client.
    Message(String),
    Disconnected,
}

/// The debuggee as `/json/list` describes it.
#[derive(Debug, Clone)]
pub struct Target {
    pub title: String,
    /// `file://` URL of the entry module.
    pub url: String,
}

pub struct InspectorServer {
    listener: TcpListener,
    addr: SocketAddr,
    token: String,
    target: Target,
}

impl InspectorServer {
    /// Binds `addr` (port 0 picks a free one) and draws the session token.
    pub fn bind(addr: SocketAddr, target: Target) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let addr = listener.local_addr()?;
        Ok(Self {
            listener,
            addr,
            token: random_token()?,
            target,
        })
    }

    pub fn ws_url(&self) -> String {
        format!("ws://{}/{}", self.addr, self.token)
    }

    /// Accepts connections on a background thread for the rest of the process.
    pub fn serve(self, on_event: impl Fn(Event) + Send + Sync + 'static) {
        let shared = Arc::new(Shared {
            on_event: Box::new(on_event),
            busy: AtomicBool::new(false),
            server: self.describe(),
        });
        let listener = self.listener;
        std::thread::Builder::new()
            .name("velyr-inspector".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let shared = Arc::clone(&shared);
                    std::thread::spawn(move || {
                        let _ = handle(stream, &shared);
                    });
                }
            })
            .expect("spawn the inspector thread");
    }

    fn describe(&self) -> Described {
        let host = self.addr.to_string();
        let target = &self.target;
        let list = serde_json::json!([{
            "description": "velyr instance",
            "devtoolsFrontendUrl": format!(
                "devtools://devtools/bundled/js_app.html?experiments=true&v8only=true&ws={host}/{}",
                self.token
            ),
            "id": self.token,
            "title": target.title,
            "type": "node",
            "url": target.url,
            "webSocketDebuggerUrl": self.ws_url(),
        }]);
        let version = serde_json::json!({
            "Browser": format!("Velyr/{}", env!("CARGO_PKG_VERSION")),
            "Protocol-Version": PROTOCOL_VERSION,
        });
        Described {
            token: self.token.clone(),
            list: list.to_string(),
            version: version.to_string(),
        }
    }
}

struct Shared {
    on_event: Box<dyn Fn(Event) + Send + Sync>,
    /// One session at a time.
    busy: AtomicBool,
    server: Described,
}

struct Described {
    token: String,
    list: String,
    version: String,
}

struct Request {
    path: String,
    host: Option<String>,
    websocket_key: Option<String>,
}

fn handle(mut stream: TcpStream, shared: &Shared) -> io::Result<()> {
    let (head, rest) = read_head(&mut stream)?;
    let Some(request) = parse_request(&head) else {
        return respond(&mut stream, "400 Bad Request", "text/plain", "bad request");
    };
    if !request.host.as_deref().is_some_and(is_allowed_host) {
        return respond(
            &mut stream,
            "403 Forbidden",
            "text/plain",
            "host not allowed",
        );
    }
    if let Some(key) = &request.websocket_key {
        return upgrade(stream, rest, &request.path, key, shared);
    }
    match request.path.as_str() {
        "/json/version" => respond(
            &mut stream,
            "200 OK",
            "application/json",
            &shared.server.version,
        ),
        "/json" | "/json/list" => respond(
            &mut stream,
            "200 OK",
            "application/json",
            &shared.server.list,
        ),
        _ => respond(&mut stream, "404 Not Found", "text/plain", "not found"),
    }
}

/// Reads up to the blank line; returns the head and any bytes already read past it.
fn read_head(stream: &mut TcpStream) -> io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(at + 4);
            return Ok((String::from_utf8_lossy(&buf).into_owned(), rest));
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn parse_request(head: &str) -> Option<Request> {
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    if request_line.next()? != "GET" {
        return None;
    }
    let path = request_line.next()?.to_owned();
    let mut request = Request {
        path,
        host: None,
        websocket_key: None,
    };
    for (name, value) in lines.filter_map(|line| line.split_once(':')) {
        let value = value.trim().to_owned();
        match name.trim().to_ascii_lowercase().as_str() {
            "host" => request.host = Some(value),
            "sec-websocket-key" => request.websocket_key = Some(value),
            _ => {}
        }
    }
    Some(request)
}

/// An IP literal (with or without port) or `localhost`; never a DNS name an attacker controls.
fn is_allowed_host(host: &str) -> bool {
    if host.parse::<SocketAddr>().is_ok() || host.parse::<IpAddr>().is_ok() {
        return true;
    }
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    name.eq_ignore_ascii_case("localhost")
        || name.trim_matches(['[', ']']).parse::<IpAddr>().is_ok()
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn upgrade(
    mut stream: TcpStream,
    rest: Vec<u8>,
    path: &str,
    key: &str,
    shared: &Shared,
) -> io::Result<()> {
    let token = path.strip_prefix('/').unwrap_or_default();
    if !constant_time_eq(token.as_bytes(), shared.server.token.as_bytes()) {
        return respond(&mut stream, "404 Not Found", "text/plain", "not found");
    }
    if shared.busy.swap(true, Ordering::AcqRel) {
        return respond(
            &mut stream,
            "409 Conflict",
            "text/plain",
            "a debugger is already attached",
        );
    }
    write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        derive_accept_key(key.as_bytes())
    )?;
    stream.set_read_timeout(Some(POLL_INTERVAL))?;
    let socket = WebSocket::from_partially_read(stream, rest, Role::Server, None);
    let result = run_session(socket, shared);
    shared.busy.store(false, Ordering::Release);
    result
}

fn run_session(mut socket: WebSocket<TcpStream>, shared: &Shared) -> io::Result<()> {
    let (outgoing, from_engine) = mpsc::channel();
    (shared.on_event)(Event::Connected(outgoing));
    let result = loop {
        if let Err(err) = from_engine
            .try_iter()
            .try_for_each(|text| socket.send(Message::text(text)))
        {
            break Err(err);
        }
        match socket.read() {
            Ok(Message::Text(text)) => (shared.on_event)(Event::Message(text.as_str().to_owned())),
            Ok(Message::Close(_)) => break Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(err)) if is_timeout(&err) => {}
            Err(err) => break Err(err),
        }
    };
    (shared.on_event)(Event::Disconnected);
    result.or_else(|err| match err {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => Ok(()),
        tungstenite::Error::Io(err) => Err(err),
        other => Err(io::Error::other(other)),
    })
}

fn is_timeout(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A `//# sourceMappingURL` comment embedding `map_json`, so a debugger maps the generated code
/// back to its source without reading any file.
pub fn inline_source_map_comment(map_json: &str) -> String {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(map_json);
    format!("\n//# sourceMappingURL=data:application/json;base64,{encoded}")
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_must_be_ip_literals_or_localhost() {
        for host in [
            "127.0.0.1:9229",
            "127.0.0.1",
            "[::1]:9229",
            "localhost:9229",
            "LOCALHOST",
        ] {
            assert!(is_allowed_host(host), "{host}");
        }
        for host in ["evil.example:9229", "evil.example", "127.0.0.1.nip.io:9229"] {
            assert!(!is_allowed_host(host), "{host}");
        }
    }

    #[test]
    fn parses_the_path_host_and_websocket_key() {
        let request = parse_request(
            "GET /abc HTTP/1.1\r\nHost: 127.0.0.1:1\r\nSec-WebSocket-Key: k==\r\nUpgrade: websocket",
        )
        .unwrap();
        assert_eq!(request.path, "/abc");
        assert_eq!(request.host.as_deref(), Some("127.0.0.1:1"));
        assert_eq!(request.websocket_key.as_deref(), Some("k=="));
        assert!(parse_request("POST /json HTTP/1.1").is_none());
    }

    #[test]
    fn inline_source_maps_are_base64_data_urls() {
        assert_eq!(
            inline_source_map_comment("{}"),
            "\n//# sourceMappingURL=data:application/json;base64,e30="
        );
    }

    #[test]
    fn tokens_are_128_bit_hex_and_differ() {
        let (a, b) = (random_token().unwrap(), random_token().unwrap());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}

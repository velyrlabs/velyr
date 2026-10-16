//! `fetch`, `Headers`, `Request` and `Response` on V8 under the real event loop, against a raw
//! TCP HTTP/1.1 server written in the test.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_io_net::{Resolver, install_net_with_resolver, net_host_functions};
use velyr_runtime_core::{RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{Capability, CapabilitySet, HostPattern, PortRange};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, install_url,
    timer_host_functions, url_host_functions,
};
use velyr_web_fetch::{fetch_host_functions, install_web_fetch};
use velyr_web_streams::{flate_host_functions, install_web_streams};

// `run_file` runs on its own thread, so the probe records into a process-wide log.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());

fn record(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("record expects a string".into()));
    };
    LOG.lock().unwrap().push(text.to_string_lossy());
    Ok(Value::Undefined)
}

fn localhost_resolver() -> Resolver {
    Arc::new(|_host, port| {
        Box::pin(async move { Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]) })
    })
}

/// A request as the test server saw it.
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn read_head(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).unwrap_or(0) == 0 {
            break;
        }
        buf.push(byte[0]);
    }
    (String::from_utf8_lossy(&buf).into_owned(), buf)
}

fn read_chunked(stream: &mut TcpStream) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while !line.ends_with(b"\r\n") {
            stream.read_exact(&mut byte).unwrap();
            line.push(byte[0]);
        }
        let size = usize::from_str_radix(String::from_utf8_lossy(&line).trim(), 16).unwrap();
        let mut chunk = vec![0u8; size + 2];
        stream.read_exact(&mut chunk).unwrap();
        if size == 0 {
            return body;
        }
        body.extend_from_slice(&chunk[..size]);
    }
}

fn read_request(stream: &mut TcpStream) -> Seen {
    let (head, _) = read_head(stream);
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (method, path) = (
        first.next().unwrap_or_default().to_owned(),
        first.next().unwrap_or_default().to_owned(),
    );
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.to_owned()))
        .collect();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let body = if header("transfer-encoding").as_deref() == Some("chunked") {
        read_chunked(stream)
    } else {
        let len: usize = header("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).unwrap();
        body
    };
    Seen {
        method,
        path,
        headers,
        body,
    }
}

fn response(status: &str, headers: &[&str], body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nconnection: close\r\ncontent-length: {}\r\n",
        body.len()
    );
    for header in headers {
        out.push_str(header);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// Serves every connection on a thread with `handle`, which writes the whole reply itself.
fn serve(handle: impl Fn(Seen, &mut TcpStream) + Send + Sync + 'static) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = Arc::new(handle);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let handle = handle.clone();
            std::thread::spawn(move || {
                let seen = read_request(&mut stream);
                handle(seen, &mut stream);
            });
        }
    });
    port
}

fn reply(bytes: Vec<u8>) -> impl Fn(Seen, &mut TcpStream) + Send + Sync + 'static {
    move |_, stream| {
        let _ = stream.write_all(&bytes);
    }
}

fn run(body: &str) -> Vec<String> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let script = format!(
        "(async () => {{ {body} }})().catch((e) => record('FAIL ' + (e && e.name) + ' ' + (e && e.code) + ' ' + (e && e.message)));"
    );
    let dir = std::env::temp_dir().join(format!("velyr-web-fetch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("t{}.js", script.len()));
    std::fs::write(&path, script).unwrap();

    let permissions = CapabilitySet::builder()
        .allow(Capability::NetConnect(
            HostPattern::new("127.0.0.1"),
            PortRange {
                start: 0,
                end: 65535,
            },
        ))
        .build();
    let quotas = QuotaConfig::default();
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(url_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(net_host_functions());
    functions.extend(fetch_host_functions());
    functions.push(HostFunctionDef {
        name: "record",
        arity: 1,
        callback: record,
    });
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        capabilities: Some(permissions.clone()),
        quotas: Some(quotas.clone()),
        leak_check: true,
        leak_collector: Some(velyr_runtime_ops::take_leak_report),
        ..RuntimeConfig::default()
    };
    let result = run_file(
        V8Engine::new(),
        config,
        functions,
        BOOTSTRAP_JS,
        move |isolate, sender, timers| {
            install_op_context(isolate, 0, sender, permissions, quotas);
            install_timers(isolate, timers).unwrap();
            install_url(isolate).unwrap();
            install_events(isolate).unwrap();
            install_encoding(isolate).unwrap();
            install_web_streams(isolate).unwrap();
            install_net_with_resolver(isolate, localhost_resolver()).unwrap();
            install_web_fetch(isolate).unwrap();
        },
        path,
    );
    result.expect("script runs");
    LOG.lock().unwrap().clone()
}

#[test]
fn fetch_reads_json_from_a_local_server() {
    let port = serve(reply(response(
        "200 OK",
        &["content-type: application/json"],
        br#"{"a":1,"list":[1,2]}"#,
    )));
    let log = run(&format!(
        "const res = await fetch('http://127.0.0.1:{port}/data');
         record(res.status + ' ' + res.statusText + ' ' + res.ok + ' ' + res.url.endsWith('/data'));
         record(JSON.stringify(await res.json()));"
    ));
    assert_eq!(log, ["200 OK true true", r#"{"a":1,"list":[1,2]}"#]);
}

#[test]
fn post_sends_string_and_stream_bodies() {
    let port = serve(|seen, stream| {
        let echo = format!(
            "{} {} {}",
            seen.method,
            seen.path,
            String::from_utf8_lossy(&seen.body)
        );
        let ctype = seen
            .headers
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| v.clone());
        let tag = format!(
            "content-type: x-seen-{}",
            ctype.unwrap_or_default().replace([';', ' ', '='], "_")
        );
        let _ = stream.write_all(&response("200 OK", &[&tag], echo.as_bytes()));
    });
    let log = run(&format!(
        "const url = 'http://127.0.0.1:{port}/up';
         const a = await fetch(url, {{ method: 'POST', body: 'hello' }});
         record(await a.text() + ' | ' + a.headers.get('content-type'));
         const stream = new ReadableStream({{ start(c) {{
           c.enqueue(new TextEncoder().encode('ab')); c.enqueue(new TextEncoder().encode('cd')); c.close(); }} }});
         const b = await fetch(url, {{ method: 'PUT', body: stream, duplex: 'half' }});
         record(await b.text());
         try {{ await fetch(url, {{ method: 'PUT', body: new ReadableStream() }}); }}
         catch (e) {{ record(e.name); }}"
    ));
    assert_eq!(
        log,
        [
            "POST /up hello | x-seen-text/plain_charset_UTF-8",
            "PUT /up abcd",
            "TypeError"
        ]
    );
}

#[test]
fn redirects_follow_error_manual_and_cap_at_twenty_hops() {
    let port = serve(|seen, stream| {
        let reply = match seen.path.as_str() {
            "/a" => response("302 Found", &["location: /b"], b""),
            "/b" => response(
                "200 OK",
                &[],
                format!("{} {}", seen.method, String::from_utf8_lossy(&seen.body)).as_bytes(),
            ),
            "/post" => response("303 See Other", &["location: /b"], b""),
            _ => response("302 Found", &["location: /loop"], b""),
        };
        let _ = stream.write_all(&reply);
    });
    let log = run(&format!(
        "const base = 'http://127.0.0.1:{port}';
         const followed = await fetch(base + '/a');
         record(followed.redirected + ' ' + followed.url.endsWith('/b') + ' ' + await followed.text());
         const post = await fetch(base + '/post', {{ method: 'POST', body: 'x' }});
         record(await post.text());
         const manual = await fetch(base + '/a', {{ redirect: 'manual' }});
         record(manual.type + ' ' + manual.status);
         for (const redirect of ['error', 'follow']) {{
           try {{ await fetch(base + (redirect === 'error' ? '/a' : '/loop'), {{ redirect }}); }}
           catch (e) {{ record(redirect + ' ' + e.name); }}
         }}"
    ));
    assert_eq!(
        log,
        [
            "true true GET ",
            "GET ",
            "opaqueredirect 0",
            "error TypeError",
            "follow TypeError"
        ]
    );
}

#[test]
fn redirect_to_an_ungranted_host_is_denied() {
    let port = serve(|_, stream| {
        let _ = stream.write_all(&response(
            "302 Found",
            &[&format!("location: http://localhost:{}/", 1)],
            b"",
        ));
    });
    let log = run(&format!(
        "try {{ await fetch('http://127.0.0.1:{port}/'); }}
         catch (e) {{ record(e.code); }}"
    ));
    assert_eq!(log, ["VELYR_PERMISSION_DENIED"]);
}

#[test]
fn abort_in_the_middle_of_the_body_fails_the_stream_with_abort_error() {
    let port = serve(|_, stream| {
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nfirst");
        let _ = stream.flush();
        std::thread::sleep(Duration::from_secs(5));
    });
    let log = run(&format!(
        "const controller = new AbortController();
         const res = await fetch('http://127.0.0.1:{port}/', {{ signal: controller.signal }});
         const reader = res.body.getReader();
         record(new TextDecoder().decode((await reader.read()).value));
         const pending = reader.read().catch((e) => record('pending ' + e.name));
         controller.abort();
         await pending;
         await reader.read().catch((e) => record('after ' + e.name));
         const already = await fetch('http://127.0.0.1:{port}/', {{ signal: controller.signal }}).catch((e) => e.name);
         record(already);"
    ));
    assert_eq!(
        log,
        [
            "first",
            "pending AbortError",
            "after AbortError",
            "AbortError"
        ]
    );
}

#[test]
fn compressed_bodies_are_decoded() {
    use std::io::Write as _;
    let gzip = {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"gzip body").unwrap();
        enc.finish().unwrap()
    };
    let deflate = {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"deflate body").unwrap();
        enc.finish().unwrap()
    };
    // `printf hello | brotli | xxd -i`
    let brotli = [0x0b, 0x02, 0x80, b'h', b'e', b'l', b'l', b'o', 0x03];
    let port = serve(move |seen, stream| {
        let _ = match seen.path.as_str() {
            "/gzip" => stream.write_all(&response("200 OK", &["content-encoding: gzip"], &gzip)),
            "/deflate" => stream.write_all(&response(
                "200 OK",
                &["content-encoding: deflate"],
                &deflate,
            )),
            "/br" => stream.write_all(&response("200 OK", &["content-encoding: br"], &brotli)),
            _ => stream.write_all(&response(
                "200 OK",
                &["content-encoding: gzip"],
                b"not gzip at all",
            )),
        };
    });
    let log = run(&format!(
        "for (const kind of ['gzip', 'deflate', 'br', 'bad']) {{
           try {{ record(kind + ': ' + await (await fetch('http://127.0.0.1:{port}/' + kind)).text()); }}
           catch (e) {{ record(kind + ' failed'); }}
         }}"
    ));
    assert_eq!(
        log,
        [
            "gzip: gzip body",
            "deflate: deflate body",
            "br: hello",
            "bad failed"
        ]
    );
}

#[test]
fn data_urls_work_and_other_schemes_are_rejected() {
    let log = run("const res = await fetch('data:text/plain;base64,aGk=');
         record(res.headers.get('content-type') + ' ' + await res.text());
         record(await (await fetch('data:,a%20b')).text());
         for (const url of ['file:///etc/hosts', 'ftp://example.com/']) {
           try { await fetch(url); } catch (e) { record(e.name); }
         }");
    assert_eq!(log, ["text/plain hi", "a b", "TypeError", "TypeError"]);
}

#[test]
fn response_statics_set_type_status_and_content_type() {
    let log = run("const json = Response.json({ a: 1 });
         record(json.headers.get('content-type') + ' ' + await json.text());
         const error = Response.error();
         record(error.type + ' ' + error.status);
         const redirect = Response.redirect('http://example.com/x', 301);
         record(redirect.status + ' ' + redirect.headers.get('location'));
         try { Response.redirect('http://example.com/', 200); } catch (e) { record(e.name); }
         try { new Response(null, { status: 99 }); } catch (e) { record(e.name); }
         try { error.headers.set('a', 'b'); } catch (e) { record(e.name); }");
    assert_eq!(
        log,
        [
            "application/json {\"a\":1}",
            "error 0",
            "301 http://example.com/x",
            "RangeError",
            "RangeError",
            "TypeError"
        ]
    );
}

#[test]
fn headers_normalize_sort_and_expose_set_cookie() {
    let log = run(
        "const h = new Headers([['B', ' 2 '], ['a', '1'], ['b', '3'], ['Set-Cookie', 'x=1'], ['set-cookie', 'y=2']]);
         record([...h].map(([k, v]) => k + '=' + v).join(','));
         record(h.get('b') + ' ' + h.has('A') + ' ' + JSON.stringify(h.getSetCookie()));
         h.delete('a'); h.set('c', '4');
         record([...h.keys()].join(','));
         try { h.append('bad name', 'x'); } catch (e) { record(e.name); }
         try { h.append('x', 'a\\nb'); } catch (e) { record(e.name); }",
    );
    assert_eq!(
        log,
        [
            "a=1,b=2, 3,set-cookie=x=1,set-cookie=y=2",
            "2, 3 true [\"x=1\",\"y=2\"]",
            "b,c,set-cookie,set-cookie",
            "TypeError",
            "TypeError"
        ]
    );
}

#[test]
fn bodies_are_single_use_clonable_and_typed() {
    let log = run(
        "const res = new Response('héllo');
         const copy = res.clone();
         record(res.bodyUsed + ' ' + await res.text() + ' ' + res.bodyUsed);
         try { await res.text(); } catch (e) { record(e.name); }
         record(new TextDecoder().decode(await copy.arrayBuffer()));
         record(String((await new Response('abc').bytes()).length));
         const req = new Request('http://example.com/', { method: 'post', body: new URLSearchParams({ a: '1' }) });
         record(req.method + ' ' + req.headers.get('content-type') + ' ' + await req.clone().text());
         try { new Request('http://example.com/', { body: 'x' }); } catch (e) { record(e.name); }
         record(String(new Response(null, { status: 204 }).body));
         try { await new Response('{').json(); } catch (e) { record(e.name); }",
    );
    assert_eq!(
        log,
        [
            "false héllo true",
            "TypeError",
            "héllo",
            "3",
            "POST application/x-www-form-urlencoded;charset=UTF-8 a=1",
            "TypeError",
            "null",
            "SyntaxError"
        ]
    );
}

#[test]
fn an_unread_response_body_is_not_a_leak() {
    let port = serve(reply(response("200 OK", &[], b"never read")));
    let log = run(&format!(
        "const res = await fetch('http://127.0.0.1:{port}/');
         record(String(res.status));"
    ));
    assert_eq!(log, ["200"]);
}

#[test]
fn body_used_is_true_once_the_stream_was_disturbed() {
    let port = serve(reply(response("200 OK", &[], b"abcdef")));
    let log = run(&format!(
        "const url = 'http://127.0.0.1:{port}/';
         const cancelled = await fetch(url);
         record(String(cancelled.bodyUsed));
         await cancelled.body.cancel();
         record(String(cancelled.bodyUsed));
         const peeked = await fetch(url);
         const reader = peeked.body.getReader();
         await reader.read();
         reader.releaseLock();
         record(String(peeked.bodyUsed));
         try {{ await peeked.text(); }} catch (e) {{ record(e.name); }}"
    ));
    assert_eq!(log, ["false", "true", "true", "TypeError"]);
}

#[test]
fn cross_origin_redirects_drop_credentials_and_same_origin_ones_keep_them() {
    let echo = |seen: &Seen| {
        let names: Vec<_> = seen.headers.iter().map(|(k, _)| k.as_str()).collect();
        response("200 OK", &[], names.join(",").as_bytes())
    };
    let target = serve(move |seen, stream| {
        let _ = stream.write_all(&echo(&seen));
    });
    let origin = serve(move |seen, stream| {
        let reply = match seen.path.as_str() {
            "/echo" => echo(&seen),
            "/cross" => response(
                "302 Found",
                &[&format!("location: http://127.0.0.1:{target}/")],
                b"",
            ),
            _ => response("302 Found", &["location: /echo"], b""),
        };
        let _ = stream.write_all(&reply);
    });
    let log = run(&format!(
        "const init = {{ headers: {{ authorization: 'Bearer t', cookie: 'a=1', 'proxy-authorization': 'Basic x', 'x-keep': '1' }} }};
         const cross = await fetch('http://127.0.0.1:{origin}/cross', init);
         record('cross ' + await cross.text());
         const same = await fetch('http://127.0.0.1:{origin}/same', init);
         record('same ' + await same.text());"
    ));
    let cross = &log[0];
    assert!(cross.contains("x-keep"), "{cross}");
    for stripped in ["authorization", "cookie"] {
        assert!(!cross.contains(stripped), "{cross}");
    }
    let same = &log[1];
    for kept in ["authorization", "cookie", "proxy-authorization", "x-keep"] {
        assert!(same.contains(kept), "{same}");
    }
}

#[test]
fn user_agent_defaults_to_velyr_and_an_explicit_one_wins() {
    let port = serve(|seen, stream| {
        let agent = seen
            .headers
            .iter()
            .find(|(k, _)| k == "user-agent")
            .map_or("none", |(_, v)| v.as_str());
        let _ = stream.write_all(&response("200 OK", &[], agent.as_bytes()));
    });
    let log = run(&format!(
        "const url = 'http://127.0.0.1:{port}/';
         record(await (await fetch(url)).text());
         record(await (await fetch(url, {{ headers: {{ 'user-agent': 'custom/1' }} }})).text());"
    ));
    assert_eq!(
        log,
        [concat!("Velyr/", env!("CARGO_PKG_VERSION")), "custom/1"]
    );
}

#[test]
fn body_used_sees_a_user_stream_cancelled_or_read_directly() {
    let log = run(
        "const bytes = new Uint8Array(1);
         const cancelled = new ReadableStream({ start(c) { c.enqueue(bytes); } });
         const response = new Response(cancelled);
         record(String(response.bodyUsed));
         await cancelled.cancel();
         record(String(response.bodyUsed));
         const pulled = new ReadableStream({ pull(c) { c.enqueue(bytes); } });
         const request = new Request('http://127.0.0.1:1/', { method: 'POST', body: pulled, duplex: 'half' });
         const reader = pulled.getReader();
         await reader.read();
         reader.releaseLock();
         record(String(request.bodyUsed));",
    );
    assert_eq!(log, ["false", "true", "true"]);
}

/// A server that answers as soon as the head is in, then reads the rest of the upload. What it
/// read goes to the returned channel; `close_early` makes it hang up after the reply instead.
fn early_reply_server(close_early: bool) -> (u16, std::sync::mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        read_head(&mut stream);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nearly");
        if !close_early {
            let _ = tx.send(read_chunked(&mut stream));
        }
    });
    (port, rx)
}

#[test]
fn an_upload_goes_on_after_the_server_answered_and_the_response_was_read() {
    let (port, received) = early_reply_server(false);
    let log = run(&format!(
        "let release;
         const gate = new Promise((resolve) => {{ release = resolve; }});
         const encoder = new TextEncoder();
         let step = 0;
         const body = new ReadableStream({{ async pull(c) {{
           if (step++ === 0) {{ c.enqueue(encoder.encode('ab')); return; }}
           await gate;
           c.enqueue(encoder.encode('cd'));
           c.close();
         }} }});
         const res = await fetch('http://127.0.0.1:{port}/', {{ method: 'PUT', body, duplex: 'half' }});
         record(await res.text());
         release();"
    ));
    assert_eq!(log, ["early"]);
    let body = received.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(String::from_utf8_lossy(&body), "abcd");
}

#[test]
fn an_upload_the_server_stopped_reading_ends_with_a_clear_error() {
    let (port, _received) = early_reply_server(true);
    let log = run(&format!(
        "const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
         let cancelled;
         const done = new Promise((resolve) => {{ cancelled = resolve; }});
         const body = new ReadableStream({{
           async pull(c) {{ await sleep(5); c.enqueue(new Uint8Array(4096)); }},
           cancel(reason) {{ cancelled(String(reason?.message)); }},
         }});
         const res = await fetch('http://127.0.0.1:{port}/', {{ method: 'PUT', body, duplex: 'half' }});
         record(await res.text());
         record(await Promise.race([done, sleep(5000).then(() => 'never cancelled')]));"
    ));
    assert_eq!(log[0], "early");
    assert!(log[1].contains("stopped reading"), "{log:?}");
}

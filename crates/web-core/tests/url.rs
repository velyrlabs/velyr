//! Runs the URL bootstrap on V8 under the real event loop.
use std::path::PathBuf;
use std::sync::Mutex;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_web_core::{install_url, url_host_functions};

// run_file executes on its own thread, so the probe records into a process-wide log.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());

fn record(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("record expects a string".into()));
    };
    LOG.lock().unwrap().push(text.to_string_lossy());
    Ok(Value::Undefined)
}

fn run(script: &str) -> Result<Vec<String>, RunError> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let path: PathBuf = std::env::temp_dir().join(format!(
        "velyr-web-core-url-{}-{}.js",
        std::process::id(),
        script.len()
    ));
    std::fs::write(&path, script).unwrap();
    let mut host_functions = url_host_functions();
    host_functions.push(HostFunctionDef {
        name: "record",
        arity: 1,
        callback: record,
    });
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        ..RuntimeConfig::default()
    };
    let result = run_file(
        V8Engine::new(),
        config,
        host_functions,
        "",
        |isolate, _, _| install_url(isolate).expect("url bootstrap"),
        path.clone(),
    );
    std::fs::remove_file(&path).ok();
    result.map(|()| LOG.lock().unwrap().clone())
}

fn run_ok(script: &str) -> Vec<String> {
    run(script).expect("script runs")
}

#[test]
fn relative_reference_resolves_against_base() {
    let log = run_ok(r#"record(new URL("../a?b=1#c", "http://x.com/p/q").href);"#);
    assert_eq!(log, ["http://x.com/a?b=1#c"]);
}

#[test]
fn invalid_url_throws_type_error() {
    let log = run_ok(
        r#"try { new URL("not a url"); } catch (e) { record(e.constructor.name + " " + e.code + " " + e.input); }
           try { new URL("/path", "also bad"); } catch (e) { record(e.constructor.name + " " + e.code); }
           try { new URL("http://a.com").href = "::"; } catch (e) { record(e.constructor.name + " " + e.code); }"#,
    );
    assert_eq!(
        log,
        [
            "TypeError ERR_INVALID_URL not a url",
            "TypeError ERR_INVALID_URL",
            "TypeError ERR_INVALID_URL"
        ]
    );
}

#[test]
fn can_parse_and_parse() {
    let log = run_ok(
        r#"record(String(URL.canParse("http://a.com")));
           record(String(URL.canParse("nope")));
           record(String(URL.canParse("/x", "http://a.com")));
           record(String(URL.parse("nope")));
           record(URL.parse("/x", "http://a.com").href);"#,
    );
    assert_eq!(log, ["true", "false", "true", "null", "http://a.com/x"]);
}

#[test]
fn components_are_exposed() {
    let log = run_ok(
        r#"const u = new URL("https://user:pw@Example.com:8443/a/b?x=1#frag");
           record([u.origin, u.protocol, u.username, u.password, u.host, u.hostname, u.port,
                   u.pathname, u.search, u.hash].join("|"));
           const d = new URL("http://example.com:80/");
           record(d.port + "|" + d.host + "|" + d.search + "|" + d.hash);"#,
    );
    assert_eq!(
        log,
        [
            "https://example.com:8443|https:|user|pw|example.com:8443|example.com|8443|/a/b|?x=1|#frag",
            "|example.com||"
        ]
    );
}

#[test]
fn setters_update_href() {
    let log = run_ok(
        r#"const u = new URL("http://a.com/p?q=1#h");
           u.protocol = "https"; u.username = "me"; u.password = "s e"; u.hostname = "b.org";
           u.port = "81"; u.pathname = "/new path"; u.hash = "frag"; u.search = "z=2";
           record(u.href);
           u.host = "c.net:90"; record(u.host);
           u.port = ""; u.hash = ""; u.search = ""; record(u.href);
           u.port = "99999"; record(u.port + "|" + u.href);"#,
    );
    assert_eq!(
        log,
        [
            "https://me:s%20e@b.org:81/new%20path?z=2#frag",
            "c.net:90",
            "https://me:s%20e@c.net/new%20path",
            "|https://me:s%20e@c.net/new%20path"
        ]
    );
}

#[test]
fn href_setter_reparses_and_rejects_invalid() {
    let log = run_ok(
        r#"const u = new URL("http://a.com/?k=v");
           u.href = "http://b.com/?n=m"; record(u.href + "|" + u.searchParams.get("n"));
           try { u.href = "bad"; } catch (e) { record(e.constructor.name); }
           record(u.href);"#,
    );
    assert_eq!(
        log,
        ["http://b.com/?n=m|m", "TypeError", "http://b.com/?n=m"]
    );
}

#[test]
fn search_setter_updates_search_params() {
    let log = run_ok(
        r#"const u = new URL("http://a.com/?x=1");
           const p = u.searchParams;
           u.search = "?y=2&y=3";
           record(p.get("y") + "|" + p.getAll("y").join(",") + "|" + String(p.has("x")) + "|" + String(p === u.searchParams));"#,
    );
    assert_eq!(log, ["2|2,3|false|true"]);
}

#[test]
fn search_params_mutation_updates_url() {
    let log = run_ok(
        r#"const u = new URL("http://a.com/p#h");
           u.searchParams.append("a", "1 2");
           u.searchParams.append("b", "&=");
           record(u.search + "|" + u.href);
           u.searchParams.set("a", "z"); u.searchParams.delete("b");
           record(u.search);
           u.searchParams.delete("a");
           record(u.href);"#,
    );
    assert_eq!(
        log,
        [
            "?a=1+2&b=%26%3D|http://a.com/p?a=1+2&b=%26%3D#h",
            "?a=z",
            "http://a.com/p#h"
        ]
    );
}

#[test]
fn to_string_and_to_json_return_href() {
    let log = run_ok(
        r#"const u = new URL("http://a.com/x");
           record(u.toString() + "|" + u.toJSON() + "|" + JSON.stringify({ u }) + "|" + `${u}`);"#,
    );
    assert_eq!(
        log,
        [r#"http://a.com/x|http://a.com/x|{"u":"http://a.com/x"}|http://a.com/x"#]
    );
}

#[test]
fn search_params_init_forms() {
    let log = run_ok(
        r#"record(new URLSearchParams("?a=1&b=2&a=3").toString());
           record(new URLSearchParams([["a", "1"], ["b", "2"]]).toString());
           record(new URLSearchParams({ a: "1", b: "x y" }).toString());
           record(new URLSearchParams(new URLSearchParams("q=1")).toString());
           record(String(new URLSearchParams("a=1&b=2&a=3").size));"#,
    );
    assert_eq!(log, ["a=1&b=2&a=3", "a=1&b=2", "a=1&b=x+y", "q=1", "3"]);
}

#[test]
fn search_params_encoding_and_decoding() {
    let log = run_ok(
        r#"const p = new URLSearchParams("a=%C3%A9+b&c=%ZZ&d=%E4%BD&e=%F0%9F%98%80");
           record(p.get("a")); record(p.get("c"));
           record(String(p.get("d").charCodeAt(0).toString(16)));
           record(p.get("e") === "\u{1F600}" ? "emoji" : "bad");
           const q = new URLSearchParams();
           q.append("k", "a b*-._!'()~é\u{1F600}");
           record(q.toString());"#,
    );
    assert_eq!(
        log,
        [
            "\u{e9} b",
            "%ZZ",
            "fffd",
            "emoji",
            "k=a+b*-._%21%27%28%29%7E%C3%A9%F0%9F%98%80"
        ]
    );
}

#[test]
fn search_params_iteration_and_sort() {
    let log = run_ok(
        r#"const p = new URLSearchParams("b=2&a=1&b=1&c=3");
           p.sort();
           record(p.toString());
           record([...p].map(([k, v]) => k + v).join(","));
           record([...p.keys()].join("") + "|" + [...p.values()].join(""));
           const seen = []; p.forEach((v, k, self) => seen.push(k + v + String(self === p)));
           record(seen.join(","));
           for (const [k, v] of p.entries()) { record(k + v); break; }"#,
    );
    assert_eq!(
        log,
        [
            "a=1&b=2&b=1&c=3",
            "a1,b2,b1,c3",
            "abbc|1213",
            "a1true,b2true,b1true,c3true",
            "a1"
        ]
    );
}

#[test]
fn search_params_delete_and_has_with_value() {
    let log = run_ok(
        r#"const p = new URLSearchParams("a=1&a=2&b=3");
           record(String(p.has("a", "2")) + String(p.has("a", "9")));
           p.delete("a", "1"); record(p.toString());
           p.set("b", "x"); p.set("n", "y"); record(p.toString());
           record(String(p.get("missing")));"#,
    );
    assert_eq!(log, ["truefalse", "a=2&b=3", "a=2&b=x&n=y", "null"]);
}

#[test]
fn ops_are_not_visible_to_user_code() {
    let log = run_ok(r#"record(typeof op_url_parse + typeof op_url_reparse);"#);
    assert_eq!(log, ["undefinedundefined"]);
}

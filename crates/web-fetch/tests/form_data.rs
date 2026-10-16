//! `FormData`, multipart and `blob:` URLs on V8 under the real event loop. The server side of the
//! round trips is `Velyr.serve` in the same isolate.

use std::sync::Mutex;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_io_net::{Resolver, install_net_with_resolver, net_host_functions};
use velyr_runtime_core::{RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{AddrPattern, Capability, CapabilitySet, HostPattern, PortRange};
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
    std::sync::Arc::new(|_host, port| {
        Box::pin(async move { Ok(vec![std::net::SocketAddr::from(([127, 0, 0, 1], port))]) })
    })
}

fn run(body: &str) -> Vec<String> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let script = format!(
        "(async () => {{ {body} }})().catch((e) => record('FAIL ' + (e && e.name) + ' ' + (e && e.message)));"
    );
    let dir = std::env::temp_dir().join(format!("velyr-web-fetch-form-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("t{}.js", script.len()));
    std::fs::write(&path, script).unwrap();

    let any_port = PortRange {
        start: 0,
        end: 65535,
    };
    let permissions = CapabilitySet::builder()
        .allow(Capability::NetConnect(
            HostPattern::new("127.0.0.1"),
            any_port,
        ))
        .allow(Capability::NetListen(
            AddrPattern::new("127.0.0.1"),
            any_port,
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
fn form_data_with_a_file_reaches_a_velyr_server_intact() {
    let log = run(
        r#"const server = Velyr.serve({ hostname: "127.0.0.1", port: 0 }, async (request) => {
             const form = await request.formData();
             const file = form.get("upload");
             return Response.json({
               type: request.headers.get("content-type").split(";")[0],
               title: form.get("title"),
               tags: form.getAll("tag"),
               name: file.name,
               fileType: file.type,
               isFile: file instanceof File,
               bytes: Array.from(await file.bytes()),
             });
           });
           try {
           const body = new FormData();
           body.append("title", "café \"quoted\"\nline");
           body.append("tag", "a");
           body.append("tag", "b");
           body.append("upload", new File([new Uint8Array([0, 13, 10, 255, 45, 45])], "dáta.bin", { type: "application/x-test" }));
           const res = await fetch(`http://127.0.0.1:${server.addr.port}/`, { method: "POST", body });
           record(JSON.stringify(await res.json()));
           } finally { await server.shutdown(); }"#,
    );
    assert_eq!(
        log,
        [
            r#"{"type":"multipart/form-data","title":"café \"quoted\"\r\nline","tags":["a","b"],"name":"dáta.bin","fileType":"application/x-test","isFile":true,"bytes":[0,13,10,255,45,45]}"#
        ]
    );
}

#[test]
fn form_data_api_follows_the_standard() {
    let log = run(r#"const form = new FormData();
           form.append("a", "1");
           form.append("a", "2");
           form.set("b", "x");
           form.append("f", new Blob(["hi"], { type: "text/plain" }));
           form.append("g", new File(["yo"], "orig.txt"), "renamed.txt");
           record([form.get("a"), form.getAll("a").join(), form.has("b"), form.has("zz"), String(form.get("zz"))].join("|"));
           record([form.get("f").name, form.get("f") instanceof File, form.get("f").type, form.get("g").name].join("|"));
           form.set("a", "only");
           form.delete("b");
           record([...form].map(([k, v]) => k + "=" + (typeof v === "string" ? v : v.name)).join("&"));
           record([...form.keys()].join() + " " + Object.prototype.toString.call(form));
           try { form.append("x"); } catch (e) { record(e.name); }
           try { form.append("x", {}, "n"); record("no error"); } catch (e) { record(e.name); }
           form.append("y", {}); record(form.get("y"));"#);
    assert_eq!(
        log,
        [
            "1|1,2|true|false|null",
            "blob|true|text/plain|renamed.txt",
            "a=only&f=blob&g=renamed.txt",
            "a,f,g [object FormData]",
            "TypeError",
            "TypeError",
            "[object Object]",
        ]
    );
}

#[test]
fn urlencoded_and_multipart_bodies_parse_and_malformed_ones_fail() {
    let log = run(
        r#"const url = new Response("a=1&b=%C3%A9&a=2", { headers: { "content-type": "application/x-www-form-urlencoded" } });
           const parsed = await url.formData();
           record(parsed.getAll("a").join() + " " + parsed.get("b"));
           const raw = "--XX\r\nContent-Disposition: form-data; name=\"k\"\r\n\r\nv\r\n--XX--\r\n";
           const multi = await new Response(raw, { headers: { "content-type": "multipart/form-data; boundary=XX" } }).formData();
           record(multi.get("k"));
           for (const [body, type] of [["x", "text/plain"], ["--XX\r\nbroken", "multipart/form-data; boundary=XX"], ["x", "multipart/form-data"]]) {
             try { await new Response(body, { headers: { "content-type": type } }).formData(); record("no error"); }
             catch (e) { record(e.name); }
           }
           const echoed = await new Response(await Response.prototype.formData.call(new Response(raw, { headers: { "content-type": "multipart/form-data; boundary=XX" } }))).formData();
           record(echoed.get("k"));"#,
    );
    assert_eq!(
        log,
        ["1,2 é", "v", "TypeError", "TypeError", "TypeError", "v"]
    );
}

#[test]
fn blob_bodies_and_blob_urls() {
    let log = run(
        r#"const blob = new Blob(["abcdef"], { type: "text/x-test" });
           const asBody = new Response(blob);
           record(asBody.headers.get("content-type") + " " + await asBody.text());
           record(await (await new Response("zzz").blob()).text());
           const url = URL.createObjectURL(blob);
           const res = await fetch(url);
           record([res.status, res.headers.get("content-type"), res.headers.get("content-length"), await res.text()].join("|"));
           const chunks = [];
           for await (const chunk of blob.slice(1, 4).stream()) chunks.push(new TextDecoder().decode(chunk));
           record(chunks.join("+"));
           URL.revokeObjectURL(url);
           try { await fetch(url); record("no error"); } catch (e) { record(e.name); }"#,
    );
    assert_eq!(
        log,
        [
            "text/x-test abcdef",
            "zzz",
            "200|text/x-test|6|abcdef",
            "bcd",
            "TypeError"
        ]
    );
}

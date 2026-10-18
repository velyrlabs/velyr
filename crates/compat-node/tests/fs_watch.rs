//! `fs.watch`, `fs.watchFile` and `fs.promises.watch` over `Velyr.watchFs`, under the real event
//! loop. Every script is bounded by a watchdog so a missing event fails instead of hanging.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use velyr_compat_node::{
    ProcessConfig, compat_node_host_functions, configure_process, install_compat_node,
    install_process_global,
};
use velyr_engine_v8::V8Engine;
use velyr_io_fs::{fs_host_functions, install_fs};
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, install_url,
    timer_host_functions, url_host_functions,
};
use velyr_web_streams::{flate_host_functions, install_web_streams};

const WATCHDOG: Duration = Duration::from_secs(30);

const PRELUDE: &str = r#"
const node = globalThis[Symbol.for('velyr.node')];
const fs = node.require('fs');
const fsp = node.require('fs/promises');
const eq = (a, b, l) => { if (!Object.is(a, b)) throw new Error(`${l || 'eq'}: got ${String(a)}, expected ${String(b)}`); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// Fails after 5 s instead of waiting for an event that never comes.
const within = (promise, label) => Promise.race([promise, sleep(5000).then(() => { throw new Error('timeout: ' + label); })]);
"#;

fn run_script(body: &str, grant: bool) -> String {
    let dir = tempfile::tempdir().unwrap();
    let canonical = dunce::canonicalize(dir.path()).unwrap();
    let text = canonical.to_str().unwrap().to_owned();
    let entry: PathBuf = canonical.join("main.js");
    // The watched directory is a sub-directory, so the entry file itself raises no events.
    let watched = canonical.join("w");
    std::fs::create_dir(&watched).unwrap();
    let script = format!(
        "(async () => {{\nconst DIR = {:?};\n{PRELUDE}\n{body}\n}})().catch((e) => {{ const m = String((e && e.stack) || e); process.stderr.write('SCRIPT FAILED: ' + m + '\\n'); return Promise.reject(m); }});",
        watched.to_str().unwrap()
    );
    std::fs::write(&entry, script).unwrap();
    let mut builder = CapabilitySet::builder();
    if grant {
        builder = builder
            .allow(Capability::FsRead(PathPattern::new(&text)))
            .allow(Capability::FsWrite(PathPattern::new(&text)));
    }
    let permissions = builder.build();
    let quotas = QuotaConfig::default();
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(url_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(compat_node_host_functions());
    functions.extend(fs_host_functions());
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    std::thread::spawn(move || {
        std::thread::sleep(WATCHDOG);
        stop.cancel();
    });
    let result = run_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            capabilities: Some(permissions.clone()),
            quotas: Some(quotas.clone()),
            shutdown: Some(shutdown),
            ..RuntimeConfig::default()
        },
        functions,
        BOOTSTRAP_JS,
        move |isolate, sender, timers| {
            install_op_context(isolate, 0, sender, permissions, quotas);
            configure_process(
                isolate,
                ProcessConfig {
                    exec_path: "/opt/velyr/bin/velyr".into(),
                    script: None,
                    args: vec![],
                    on_exit: Some(Arc::new(|_| {})),
                },
            );
            install_timers(isolate, timers.clone()).expect("timers");
            install_events(isolate).expect("events");
            install_url(isolate).expect("url");
            install_encoding(isolate).expect("encoding");
            install_web_streams(isolate).expect("web streams");
            install_fs(isolate).expect("fs");
            install_compat_node(isolate).expect("compat node");
            let drain = install_process_global(isolate, timers).expect("process global");
            isolate.set_tick_hook(drain);
        },
        entry,
    );
    match result {
        Ok(()) => "ok".into(),
        Err(RunError::UnhandledRejection(reason)) => reason,
        Err(RunError::Uncaught(exception)) => exception.message,
        Err(other) => format!("{other:?}"),
    }
}

fn check(body: &str) {
    let (tx, rx) = std::sync::mpsc::channel();
    let body = body.to_owned();
    std::thread::spawn(move || {
        let _ = tx.send(run_script(&body, true));
    });
    let result = rx
        .recv_timeout(WATCHDOG * 2)
        .expect("the script did not finish before the watchdog");
    assert_eq!(result, "ok", "{result}");
}

#[test]
fn watch_reports_rename_for_a_new_file_and_change_for_an_edit() {
    check(
        r#"
        const seen = [];
        let wake;
        const watcher = fs.watch(DIR, (type, name) => { seen.push(type + ':' + name); wake?.(); });
        const next = (want) => within(new Promise((resolve) => {
          const look = () => (seen.includes(want) ? resolve() : (wake = look));
          look();
        }), want + ' in ' + seen.join());
        await sleep(150);
        fs.writeFileSync(DIR + '/a.txt', 'one');
        await next('rename:a.txt');
        await sleep(200);
        fs.writeFileSync(DIR + '/a.txt', 'two');
        await next('change:a.txt');
        fs.unlinkSync(DIR + '/a.txt');
        await sleep(200);
        watcher.close();
        const count = seen.length;
        fs.writeFileSync(DIR + '/b.txt', 'x');
        await sleep(300);
        eq(seen.length, count, 'no events after close');
    "#,
    );
}

#[test]
fn watch_emits_change_on_the_emitter_and_close_once() {
    check(
        r#"
        const watcher = fs.watch(DIR);
        const first = within(new Promise((resolve) => watcher.on('change', (type, name) => { if (name === 'e.txt') resolve([type, name]); })), 'change');
        const closed = within(new Promise((resolve) => watcher.once('close', resolve)), 'close');
        await sleep(150);
        fs.writeFileSync(DIR + '/e.txt', 'x');
        const [type, name] = await first;
        eq(type === 'rename' || type === 'change', true, 'type');
        eq(name, 'e.txt', 'filename');
        watcher.close();
        watcher.close();
        await closed;
    "#,
    );
}

#[test]
fn promises_watch_is_an_async_iterator_of_event_type_and_filename() {
    check(
        r#"
        const ac = new AbortController();
        const it = fsp.watch(DIR, { signal: ac.signal })[Symbol.asyncIterator]();
        const pending = within(it.next(), 'first event');
        await sleep(150);
        fs.writeFileSync(DIR + '/p.txt', 'x');
        const { value, done } = await pending;
        eq(done, false, 'done');
        eq(value.filename, 'p.txt', 'filename');
        eq(value.eventType, 'rename', 'eventType');
        ac.abort();
        let aborted = 'none';
        try { await within(it.next(), 'abort'); } catch (e) { aborted = e.name; }
        eq(aborted, 'AbortError', 'abort');
    "#,
    );
}

#[test]
fn watch_file_calls_the_listener_with_current_and_previous_stats() {
    check(
        r#"
        const file = DIR + '/f.txt';
        fs.writeFileSync(file, 'one');
        await sleep(250);
        const got = within(new Promise((resolve) => fs.watchFile(file, { interval: 50 }, (curr, prev) => resolve([curr, prev]))), 'listener');
        await sleep(150);
        fs.writeFileSync(file, 'three');
        const [curr, prev] = await got;
        eq(prev.size, 3, 'prev');
        eq(curr.size, 5, 'curr');
        fs.unwatchFile(file);
        let calls = 0;
        const again = () => { calls++; };
        fs.watchFile(file, again);
        fs.unwatchFile(file, again);
        fs.writeFileSync(file, 'four');
        await sleep(300);
        eq(calls, 0, 'removed listener');
    "#,
    );
}

#[test]
fn watching_without_fs_read_is_denied() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_script(
            r#"
            const code = (fn) => { try { fn(); } catch (e) { return e.code + '/' + e.cause?.code; } return 'no error'; };
            eq(code(() => fs.watch(DIR)), 'ERR_ACCESS_DENIED/VELYR_PERMISSION_DENIED', 'watch');
            eq(code(() => fs.watchFile(DIR + '/x', () => {})), 'ERR_ACCESS_DENIED/VELYR_PERMISSION_DENIED', 'watchFile');
            let denied = 'none';
            try { await fsp.watch(DIR)[Symbol.asyncIterator]().next(); } catch (e) { denied = e.code; }
            eq(denied, 'ERR_ACCESS_DENIED', 'promises.watch');
        "#,
            false,
        ));
    });
    let result = rx.recv_timeout(WATCHDOG * 2).expect("watchdog");
    assert_eq!(result, "ok", "{result}");
}

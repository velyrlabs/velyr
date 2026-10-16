//! `node:child_process` on V8 under the real event loop, over io-process. Each script runs with an
//! explicit set of `ProcessSpawn` grants (what `--allow-run=...` expands to) and only macOS/Linux
//! base binaries. Every run is bounded by a watchdog, so a regression that hangs fails the test.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use velyr_compat_node::{
    ProcessConfig, compat_node_host_functions, configure_process, install_compat_node,
    install_process_global,
};
use velyr_engine_v8::V8Engine;
use velyr_io_process::{install_command, process_host_functions};
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern, ProgramPattern};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, install_url,
    timer_host_functions, url_host_functions,
};
use velyr_web_streams::{flate_host_functions, install_web_streams};

const WATCHDOG: Duration = Duration::from_secs(60);
const EXEC_PATH: &str = "/opt/velyr/bin/velyr";

const PRELUDE: &str = r#"
const cp = globalThis[Symbol.for('velyr.node')].require('child_process');
const eq = (a, b, l) => { if (!Object.is(a, b)) throw new Error(`${l || 'eq'}: got ${String(a)}, expected ${String(b)}`); };
const codeOf = (fn) => { try { fn(); return 'no error'; } catch (e) { return e.code ?? String(e); } };
const once = (emitter, name) => new Promise((resolve) => emitter.once(name, (...args) => resolve(args)));
const exec = (fn, ...args) => new Promise((resolve) => fn(...args, (error, stdout, stderr) => resolve({ error, stdout, stderr })));
const SPIN = ['-c', 'while :; do echo xxxxxxxx; done'];
"#;

fn run_script(allow: &[&str], body: &str, shutdown: Option<CancellationToken>) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("main.js");
    let script = format!(
        "(async () => {{\n{PRELUDE}\n{body}\n}})().catch((e) => {{ const m = String((e && e.stack) || e); process.stderr.write('SCRIPT FAILED: ' + m + '\\n'); return Promise.reject(m); }});"
    );
    std::fs::write(&path, script).unwrap();
    let mut builder = CapabilitySet::builder();
    for program in allow {
        // "read:<path>" grants fs-read on that path; anything else is a process-spawn grant.
        builder = match program.strip_prefix("read:") {
            Some(path) => builder.allow(Capability::FsRead(PathPattern::new(path))),
            None => builder.allow(Capability::ProcessSpawn(ProgramPattern::new(*program))),
        };
    }
    let permissions = builder.build();
    let quotas = QuotaConfig::default();
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(url_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(process_host_functions());
    functions.extend(compat_node_host_functions());
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        capabilities: Some(permissions.clone()),
        quotas: Some(quotas.clone()),
        shutdown,
        ..RuntimeConfig::default()
    };
    let result = run_file(
        V8Engine::new(),
        config,
        functions,
        BOOTSTRAP_JS,
        move |isolate, sender, timers| {
            install_op_context(isolate, 0, sender, permissions, quotas);
            configure_process(
                isolate,
                ProcessConfig {
                    exec_path: EXEC_PATH.into(),
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
            install_command(isolate).expect("command");
            install_compat_node(isolate).expect("compat node");
            let drain = install_process_global(isolate, timers).expect("process global");
            isolate.set_tick_hook(drain);
        },
        path,
    );
    match result {
        Ok(()) => "ok".into(),
        Err(RunError::UnhandledRejection(reason)) => reason,
        Err(RunError::Uncaught(exception)) => exception.message,
        Err(other) => format!("{other:?}"),
    }
}

/// Runs `body` as an async function under the grants in `allow`; panics if it takes too long.
fn run_in_thread(allow: &[&str], body: &str, shutdown: Option<CancellationToken>) -> String {
    let (allow, body): (Vec<String>, String) = (
        allow.iter().map(|s| (*s).to_owned()).collect(),
        body.to_owned(),
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let allow: Vec<&str> = allow.iter().map(String::as_str).collect();
        let _ = tx.send(run_script(&allow, &body, shutdown));
    });
    rx.recv_timeout(WATCHDOG)
        .expect("the script did not finish before the watchdog")
}

fn check(allow: &[&str], body: &str) {
    assert_eq!(run_in_thread(allow, body, None), "ok");
}

#[test]
fn without_a_grant_nothing_spawns() {
    check(
        &[],
        r#"
        eq(codeOf(() => cp.execFileSync('echo', ['hi'])), 'ERR_ACCESS_DENIED', 'execFileSync');
        eq(codeOf(() => cp.execSync('echo hi')), 'ERR_ACCESS_DENIED', 'execSync');
        eq(cp.spawnSync('echo', ['hi']).error.code, 'ERR_ACCESS_DENIED', 'spawnSync');
        const child = cp.spawn('echo', ['hi']);
        const [error] = await once(child, 'error');
        eq(error.code, 'ERR_ACCESS_DENIED', 'spawn');
        eq(child.pid, undefined, 'pid');
        eq((await exec(cp.execFile, 'echo', ['hi'])).error.code, 'ERR_ACCESS_DENIED', 'execFile');
    "#,
    );
}

#[test]
fn an_allowed_program_runs_and_others_are_denied() {
    check(
        &["echo"],
        r#"
        eq(cp.execFileSync('echo', ['hi']).toString(), 'hi\n', 'sync');
        eq(cp.execFileSync('echo', ['hi'], { encoding: 'utf8' }), 'hi\n', 'encoding');
        eq(codeOf(() => cp.execFileSync('ls', ['/'])), 'ERR_ACCESS_DENIED', 'ls sync');
        const denied = cp.spawn('ls', ['/']);
        eq((await once(denied, 'error'))[0].code, 'ERR_ACCESS_DENIED', 'ls async');

        const child = cp.spawn('echo', ['async hi']);
        let out = '';
        child.stdout.on('data', (chunk) => { out += chunk; });
        const [code, signal] = await once(child, 'close');
        eq(out, 'async hi\n', 'stdout');
        eq(code, 0, 'code'); eq(signal, null, 'signal');
        eq(typeof child.pid, 'number', 'pid');

        const { error, stdout } = await exec(cp.execFile, 'echo', ['via execFile']);
        eq(error, null, 'execFile error'); eq(stdout, 'via execFile\n', 'execFile stdout');
    "#,
    );
}

#[test]
fn a_name_with_a_slash_needs_the_path_to_be_granted() {
    check(
        &["echo"],
        r#"
        eq(codeOf(() => cp.execFileSync('/bin/echo', ['hi'])), 'ERR_ACCESS_DENIED');
    "#,
    );
    check(
        &["/bin/echo"],
        r#"
        eq(cp.execFileSync('/bin/echo', ['hi']).toString(), 'hi\n', 'by path');
        eq(cp.execFileSync('echo', ['hi']).toString(), 'hi\n', 'name resolving to the granted path');
    "#,
    );
}

#[test]
fn a_missing_program_that_is_granted_is_enoent() {
    check(
        &["no-such-program-zz"],
        r#"
        eq(cp.spawnSync('no-such-program-zz').error.code, 'ENOENT', 'sync');
        const [error] = await once(cp.spawn('no-such-program-zz'), 'error');
        eq(error.code, 'ENOENT', 'async'); eq(error.syscall, 'spawn no-such-program-zz');
    "#,
    );
}

#[test]
fn exec_needs_the_shell_to_be_granted() {
    check(
        &["echo"],
        r#"
        eq(codeOf(() => cp.execSync('echo hi')), 'ERR_ACCESS_DENIED', 'execSync');
        eq((await exec(cp.exec, 'echo hi')).error.code, 'ERR_ACCESS_DENIED', 'exec');
        eq(codeOf(() => cp.execFileSync('echo', ['hi'], { shell: true })), 'ERR_ACCESS_DENIED', 'shell option');
    "#,
    );
    check(
        &["/bin/sh"],
        r#"
        eq(cp.execSync('echo hi').toString(), 'hi\n', 'execSync');
        const { error, stdout } = await exec(cp.exec, 'echo hi && echo there');
        eq(error, null); eq(stdout, 'hi\nthere\n', 'exec');
        const failed = await exec(cp.exec, 'echo oops >&2; exit 3');
        eq(failed.error.code, 3, 'exit code'); eq(failed.stderr, 'oops\n', 'stderr');
    "#,
    );
}

#[test]
fn the_environment_is_exactly_what_was_passed() {
    check(
        &["sh"],
        r#"
        const script = ['-c', 'echo "${HOME-unset}:${X-unset}"'];
        eq(cp.execFileSync('sh', script, { env: {} }).toString(), 'unset:unset\n', 'empty');
        eq(cp.execFileSync('sh', script, { env: { X: '1' } }).toString(), 'unset:1\n', 'explicit');
        const { stdout } = await exec(cp.execFile, 'sh', script, { env: { X: '2' } });
        eq(stdout, 'unset:2\n', 'async');
    "#,
    );
}

#[test]
fn sync_runs_feed_input_and_honor_timeouts() {
    check(
        &["cat", "sleep"],
        r#"
        eq(cp.execFileSync('cat', [], { input: 'round trip' }).toString(), 'round trip', 'input');
        const slow = cp.spawnSync('sleep', ['7201'], { timeout: 200 });
        eq(slow.error.code, 'ETIMEDOUT', 'timeout'); eq(slow.signal, 'SIGKILL', 'signal');
    "#,
    );
}

#[test]
fn output_over_max_buffer_fails_fast() {
    let started = Instant::now();
    check(
        &["sh"],
        r#"
        eq(codeOf(() => cp.execFileSync('sh', SPIN, { maxBuffer: 4096 })), 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER', 'sync');
        eq(cp.spawnSync('sh', SPIN, { maxBuffer: 1024 }).error.code, 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER', 'spawnSync');
        const { error, stdout } = await exec(cp.execFile, 'sh', SPIN, { maxBuffer: 4096 });
        eq(error.code, 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER', 'async');
        eq(stdout.length <= 4096, true, 'kept at most maxBuffer');
        eq(codeOf(() => cp.execFileSync('sh', SPIN, { maxBuffer: Infinity })), 'ERR_OUT_OF_RANGE', 'unbounded');
    "#,
    );
    assert!(started.elapsed() < WATCHDOG);
}

#[test]
fn kill_is_sigkill_and_nothing_else_is_supported() {
    check(
        &["sleep"],
        r#"
        const child = cp.spawn('sleep', ['7202']);
        eq(codeOf(() => child.kill('SIGTERM')), 'ERR_VELYR_UNSUPPORTED', 'SIGTERM');
        eq(child.kill(), true, 'kill');
        const [code, signal] = await once(child, 'exit');
        eq(code, null); eq(signal, 'SIGKILL');
        eq(child.killed, true, 'killed');
        eq(codeOf(() => cp.spawn('sleep', ['1'], { detached: true })), 'ERR_VELYR_UNSUPPORTED', 'detached');
        eq(codeOf(() => cp.spawn('sleep', ['1'], { stdio: 'ipc' })), 'ERR_VELYR_UNSUPPORTED', 'ipc');
    "#,
    );
}

#[test]
fn fork_is_unsupported_even_with_every_grant() {
    // `fork('x.js', ['-A'])` would start the runtime with all permissions: it throws instead.
    check(
        &["*"],
        r#"
        eq(codeOf(() => cp.fork('/nonexistent/module.js')), 'ERR_VELYR_UNSUPPORTED', 'fork');
        eq(codeOf(() => cp.fork('x.js', ['-A'])), 'ERR_VELYR_UNSUPPORTED', 'fork -A');
        eq(codeOf(() => cp.fork('x.js', ['-A'], { silent: true })), 'ERR_VELYR_UNSUPPORTED', 'silent');
    "#,
    );
}

#[test]
fn loader_variables_never_reach_a_child() {
    check(
        &["sh"],
        r#"
        const script = ['-c', 'echo ${LD_PRELOAD-unset}'];
        eq(codeOf(() => cp.execFileSync('sh', script, { env: { LD_PRELOAD: '/tmp/x.so' } })), 'ERR_ACCESS_DENIED', 'LD_PRELOAD');
        eq(codeOf(() => cp.execFileSync('sh', script, { env: { DYLD_INSERT_LIBRARIES: '/tmp/x' } })), 'ERR_ACCESS_DENIED', 'DYLD');
        eq(cp.spawnSync('sh', script, { env: { LD_LIBRARY_PATH: '/tmp' } }).error.code, 'ERR_ACCESS_DENIED', 'spawnSync');
        const [error] = await once(cp.spawn('sh', script, { env: { DYLD_FRAMEWORK_PATH: '/tmp' } }), 'error');
        eq(error.code, 'ERR_ACCESS_DENIED', 'async');
    "#,
    );
}

#[test]
fn max_buffer_has_a_64_mib_ceiling() {
    check(
        &["sh", "echo"],
        r#"
        const big = { maxBuffer: 2 ** 52 };
        eq(codeOf(() => cp.execFileSync('echo', ['hi'], big)), 'ERR_OUT_OF_RANGE', 'sync');
        eq(codeOf(() => cp.spawnSync('echo', ['hi'], big)), 'ERR_OUT_OF_RANGE', 'spawnSync');
        eq(codeOf(() => cp.execFile('echo', ['hi'], big, () => {})), 'ERR_OUT_OF_RANGE', 'async');
        eq(codeOf(() => cp.exec('echo hi', big, () => {})), 'ERR_OUT_OF_RANGE', 'exec');
        eq(cp.execFileSync('echo', ['hi'], { maxBuffer: 64 * 1024 * 1024 }).toString(), 'hi\n', 'at the ceiling');
    "#,
    );
}

#[test]
fn cwd_needs_fs_read() {
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let dir = dir.to_str().unwrap().to_owned();
    let body = format!(
        r#"
        eq(codeOf(() => cp.execFileSync('echo', ['hi'], {{ cwd: '/no/such/dir-zz' }})), 'ERR_ACCESS_DENIED', 'missing');
        eq(codeOf(() => cp.execFileSync('echo', ['hi'], {{ cwd: {dir:?} }})), 'ERR_ACCESS_DENIED', 'existing');
        const [error] = await once(cp.spawn('echo', ['hi'], {{ cwd: {dir:?} }}), 'error');
        eq(error.code, 'ERR_ACCESS_DENIED', 'async');
    "#
    );
    check(&["echo"], &body);
    let grant = format!("read:{dir}");
    check(
        &["echo", &grant],
        &format!(
            "eq(cp.execFileSync('echo', ['hi'], {{ cwd: {dir:?} }}).toString(), 'hi\\n', 'granted');"
        ),
    );
}

#[test]
fn a_blocked_sync_call_ends_when_the_isolate_is_shut_down() {
    let needle = "sleep 7204";
    assert!(!alive(needle));
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let watcher = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !alive(needle) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        stop.cancel();
    });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_script(
            &["sleep"],
            "cp.spawnSync('sleep', ['7204']);",
            Some(shutdown),
        ));
    });
    rx.recv_timeout(Duration::from_secs(20))
        .expect("spawnSync kept the isolate blocked after shutdown");
    watcher.join().unwrap();
    assert!(!alive(needle), "the child outlived the call");
}

#[test]
fn stdin_is_piped_to_the_child() {
    check(
        &["cat"],
        r#"
        const child = cp.spawn('cat');
        let out = '';
        child.stdout.on('data', (chunk) => { out += chunk; });
        child.stdin.write('hello ');
        child.stdin.end('world');
        await once(child, 'close');
        eq(out, 'hello world');
        const ignored = cp.spawn('cat', { stdio: 'ignore' });
        eq(ignored.stdout, null, 'ignored');
        const [code] = await once(ignored, 'close');
        eq(code, 0, 'cat on /dev/null ends');
    "#,
    );
}

fn alive(needle: &str) -> bool {
    let out = std::process::Command::new("/bin/ps")
        .args(["-axo", "stat=,command="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.contains(needle) && !line.trim_start().starts_with('Z'))
}

#[test]
fn a_child_dies_when_the_isolate_ends() {
    let needle = "sleep 7203";
    assert!(!alive(needle));
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let watcher = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !alive(needle) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        stop.cancel();
    });
    let body = "await once(cp.spawn('sleep', ['7203']), 'close');";
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_script(&["sleep"], body, Some(shutdown)));
    });
    rx.recv_timeout(WATCHDOG)
        .expect("the isolate did not shut down");
    watcher.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(needle) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(needle), "the child outlived its isolate");
}

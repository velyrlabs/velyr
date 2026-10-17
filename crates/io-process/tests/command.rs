#![cfg(unix)] // velyr-io-process spawns children only on unix; Windows returns Unsupported.
//! `Velyr.Command` on V8 under the real event loop, with capability sets built directly. Every run
//! has a watchdog so a hang fails the test.

use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_io_process::{install_command, process_host_functions};
use velyr_runtime_core::{RuntimeConfig, run_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern, ProgramPattern};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, timer_host_functions,
};
use velyr_web_streams::{flate_host_functions, install_web_streams};

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

fn run(allow: &[&str], processes: Option<u64>, body: impl Into<String>) -> Vec<String> {
    let allow: Vec<String> = allow.iter().map(|s| (*s).to_owned()).collect();
    let body: String = body.into();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_inner(&allow, processes, &body, None));
    });
    rx.recv_timeout(Duration::from_secs(60))
        .expect("script timed out")
}

fn run_inner(
    allow: &[String],
    processes: Option<u64>,
    body: &str,
    shutdown: Option<CancellationToken>,
) -> Vec<String> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOG.lock().unwrap().clear();
    let script = format!(
        "(async () => {{ {body} }})().catch((e) => record('FAIL ' + (e && e.stack || e)));"
    );
    let dir = std::env::temp_dir().join(format!("velyr-io-process-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("t{}.js", script.len()));
    std::fs::write(&path, script).unwrap();

    let shutdown_requested = shutdown.is_some();
    let mut builder = CapabilitySet::builder();
    for program in allow {
        // "read:<path>" grants fs-read on that path; anything else is a process-spawn grant.
        builder = match program.strip_prefix("read:") {
            Some(path) => builder.allow(Capability::FsRead(PathPattern::new(path))),
            None => builder.allow(Capability::ProcessSpawn(ProgramPattern::new(
                program.as_str(),
            ))),
        };
    }
    let permissions = builder.build();
    let quotas = QuotaConfig {
        processes,
        ..QuotaConfig::default()
    };
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(timer_host_functions());
    functions.extend(encoding_host_functions());
    functions.extend(flate_host_functions());
    functions.extend(process_host_functions());
    functions.push(HostFunctionDef {
        name: "record",
        arity: 1,
        callback: record,
    });
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
            install_timers(isolate, timers).unwrap();
            install_events(isolate).unwrap();
            install_encoding(isolate).unwrap();
            install_web_streams(isolate).unwrap();
            install_command(isolate).unwrap();
        },
        path,
    );
    if shutdown_requested {
        assert!(result.is_err(), "a shut-down run is reported as terminated");
    } else {
        result.expect("script runs");
    }
    LOG.lock().unwrap().clone()
}

const CATCH: &str = "const code = async (f) => { try { await f(); return 'ok'; } catch (e) { return e.code ?? String(e); } };";

#[test]
fn without_a_grant_spawning_is_denied() {
    let log = run(
        &[],
        None,
        format!(
            "{CATCH} record(await code(() => new Velyr.Command('echo', {{ args: ['hi'] }}).output()));"
        ),
    );
    assert_eq!(log, ["VELYR_PERMISSION_DENIED"]);
}

#[test]
fn a_granted_program_runs_and_another_is_denied() {
    let log = run(
        &["echo"],
        None,
        format!(
            "{CATCH}
         const out = await new Velyr.Command('echo', {{ args: ['hi'] }}).output();
         record(`${{out.success}} ${{new TextDecoder().decode(out.stdout)}}`.trim());
         record(await code(() => new Velyr.Command('ls').output()));"
        ),
    );
    assert_eq!(log, ["true hi", "VELYR_PERMISSION_DENIED"]);
}

#[test]
fn the_environment_is_exactly_what_was_passed() {
    let log = run(
        &["sh"],
        None,
        "const run = async (env) => new TextDecoder().decode((await new Velyr.Command('sh', { args: ['-c', 'echo \"${HOME-unset}:${X-unset}\"'], env }).output()).stdout).trim();
         record(await run({})); record(await run({ X: '1' }));",
    );
    assert_eq!(log, ["unset:unset", "unset:1"]);
}

#[test]
fn output_over_max_buffer_fails_fast() {
    let started = Instant::now();
    let log = run(&["sh"], None, format!(
        "{CATCH}
         const cmd = new Velyr.Command('sh', {{ args: ['-c', 'while :; do echo xxxxxxxx; done'], maxBuffer: 4096 }});
         record(await code(() => cmd.output()));"
    ));
    assert_eq!(log, ["VELYR_QUOTA_EXCEEDED"]);
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[test]
fn stdin_is_piped_to_the_child_and_kill_reports_sigkill() {
    let log = run(
        &["cat", "sleep"],
        None,
        "const child = new Velyr.Command('cat', { stdin: 'piped', stdout: 'piped' }).spawn();
         const writer = child.stdin.getWriter();
         await writer.write(new TextEncoder().encode('round trip'));
         await writer.close();
         let text = '';
         for await (const chunk of child.stdout) text += new TextDecoder().decode(chunk);
         record(text + ' ' + (await child.status).success);
         const sleeper = new Velyr.Command('sleep', { args: ['7104'] }).spawn();
         sleeper.kill();
         const status = await sleeper.status;
         record(`${status.code} ${status.signal}`);",
    );
    assert_eq!(log, ["round trip true", "null SIGKILL"]);
}

#[test]
fn only_sigkill_can_be_sent() {
    let log = run(
        &["sleep"],
        None,
        format!(
            "{CATCH}
         const child = new Velyr.Command('sleep', {{ args: ['7105'] }}).spawn();
         record(await code(async () => child.kill('SIGTERM')));
         child.kill(); await child.status;"
        ),
    );
    assert_eq!(log, ["VELYR_UNSUPPORTED"]);
}

#[test]
fn the_process_quota_limits_live_children() {
    let log = run(
        &["sleep"],
        Some(1),
        format!(
            "{CATCH}
         const first = new Velyr.Command('sleep', {{ args: ['7106'] }}).spawn();
         record(await code(async () => new Velyr.Command('sleep', {{ args: ['7106'] }}).spawn()));
         first.kill(); await first.status;
         const again = new Velyr.Command('sleep', {{ args: ['7106'] }}).spawn();
         again.kill(); await again.status;
         record('reusable');"
        ),
    );
    assert_eq!(log, ["VELYR_QUOTA_EXCEEDED", "reusable"]);
}

#[test]
fn the_runtimes_own_executable_cannot_be_started_even_when_granted() {
    let me = std::env::current_exe().unwrap();
    let me = me.to_str().unwrap();
    let log = run(
        &[me],
        None,
        format!(
            "try {{ await new Velyr.Command({me:?}, {{ args: ['--version'] }}).output(); record('started'); }}
             catch (e) {{ record(e.code + ' ' + /runtime/i.test(e.message)); }}"
        ),
    );
    assert_eq!(log, ["VELYR_PERMISSION_DENIED true"]);
}

#[test]
fn max_buffer_has_a_ceiling_and_loader_variables_are_refused() {
    let log = run(
        &["echo"],
        None,
        format!(
            "{CATCH}
             record(await code(async () => new Velyr.Command('echo', {{ maxBuffer: 2 ** 52 }}).output()));
             record(await code(async () => new Velyr.Command('echo', {{ env: {{ LD_PRELOAD: '/tmp/x.so' }} }}).output()));
             record(await code(async () => new Velyr.Command('echo', {{ env: {{ DYLD_INSERT_LIBRARIES: '/tmp/x' }} }}).spawn()));"
        ),
    );
    assert_eq!(
        log,
        [
            "ERR_OUT_OF_RANGE",
            "VELYR_PERMISSION_DENIED",
            "VELYR_PERMISSION_DENIED"
        ]
    );
}

#[test]
fn cwd_needs_fs_read_and_is_decided_before_existence() {
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let dir = dir.to_str().unwrap().to_owned();
    let script = format!(
        "{CATCH}
         const run = (cwd) => code(() => new Velyr.Command('echo', {{ cwd }}).output());
         record(await run('/no/such/dir-zz')); record(await run({dir:?}));"
    );
    // Without fs-read, a missing and an existing directory look the same.
    let log = run(&["echo"], None, script.clone());
    assert_eq!(log, ["VELYR_PERMISSION_DENIED", "VELYR_PERMISSION_DENIED"]);
    // With it, the existing one works.
    let grant = format!("read:{dir}");
    let log = run(&["echo", &grant], None, script);
    assert_eq!(log, ["VELYR_PERMISSION_DENIED", "ok"]);
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
    let needle = "sleep 7107";
    assert!(!alive(needle));
    let shutdown = CancellationToken::new();
    // The script spawns a long-lived child and keeps waiting on it; the host then shuts the isolate
    // down (the pending wait would otherwise keep the loop alive for hours).
    let stop = shutdown.clone();
    let watcher = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !alive(needle) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        stop.cancel();
    });
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let body = "await new Velyr.Command('sleep', { args: ['7107'] }).spawn().status;";
        let _ = tx.send(run_inner(&["sleep".to_owned()], None, body, Some(shutdown)));
    });
    rx.recv_timeout(Duration::from_secs(60))
        .expect("the isolate did not shut down");
    watcher.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(needle) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(needle), "the child outlived its isolate");
}

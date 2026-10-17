//! `install_node_lazy` (VELYR-223): the compat-node bootstrap is evaluated on first use of
//! anything node, and the globals end up exactly as the eager install leaves them.
use std::sync::Mutex;

use velyr_compat_node::{
    compat_node_host_functions, emit_before_exit, emit_exit, install_compat_node,
    install_node_lazy, install_process_global,
};
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Isolate, Source, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_ops::install_op_context;
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_timers, install_url,
    timer_host_functions, url_host_functions,
};

static REPORT: Mutex<String> = Mutex::new(String::new());
// One run at a time: both tests below write the report.
static SERIAL: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Eager,
    Lazy,
}

fn op_report(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    if let Some(Value::String(text)) = args.first() {
        *REPORT.lock().unwrap() = text.to_string_lossy();
    }
    Ok(Value::Undefined)
}

/// Runs `body` (an async function body) and returns the run result and what it reported.
/// `fake_commonjs` installs the stand-in for compat-commonjs' hook before the node install.
fn run(mode: Mode, fake_commonjs: bool, emit: bool, body: &str) -> (String, String) {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    REPORT.lock().unwrap().clear();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.js");
    std::fs::write(
        &path,
        format!("(async () => {{\n{body}\n}})().catch((e) => Promise.reject(String((e && e.stack) || e)));"),
    )
    .unwrap();
    let mut host_functions = timer_host_functions();
    host_functions.extend(url_host_functions());
    host_functions.extend(encoding_host_functions());
    host_functions.extend(compat_node_host_functions());
    host_functions.push(HostFunctionDef {
        name: "report",
        arity: 1,
        callback: op_report,
    });
    let result = run_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            ..RuntimeConfig::default()
        },
        host_functions,
        "",
        move |isolate, sender, timers| {
            let permissions = velyr_runtime_permissions::CapabilitySet::builder().build();
            install_op_context(isolate, 0, sender, permissions, QuotaConfig::default());
            install_timers(isolate, timers.clone()).expect("timers");
            install_events(isolate).expect("events");
            install_url(isolate).expect("url");
            install_encoding(isolate).expect("encoding");
            if fake_commonjs {
                let hook = Source {
                    name: "fake-commonjs".into(),
                    text: r#"Object.defineProperty(globalThis, Symbol.for("velyr.commonjs"),
                        { value: { builtins: new Map(), prefixOnly: new Set() } });"#
                        .into(),
                };
                isolate.eval_script(&hook).expect("hook");
            }
            let drain = match mode {
                Mode::Eager => {
                    install_compat_node(isolate).expect("compat node");
                    velyr_compat_node::install_buffer_global(isolate).expect("buffer");
                    install_process_global(isolate, timers).expect("process global")
                }
                Mode::Lazy => install_node_lazy(isolate, timers).expect("lazy node"),
            };
            isolate.set_tick_hook(drain);
            if emit {
                emit_before_exit(isolate, 0).unwrap();
                emit_exit(isolate, 0).unwrap();
            }
        },
        path,
    );
    let outcome = match result {
        Ok(()) => "ok".into(),
        Err(RunError::UnhandledRejection(reason)) => reason,
        Err(RunError::Uncaught(exception)) => exception.message,
        Err(other) => format!("{other:?}"),
    };
    (outcome, REPORT.lock().unwrap().clone())
}

const NODE_SYMBOL: &str = r#"Symbol.for("velyr.node")"#;

#[test]
fn a_script_that_uses_nothing_node_never_evaluates_compat_node() {
    let body = format!(
        r#"
        const d = Object.getOwnPropertyDescriptor(globalThis, {NODE_SYMBOL});
        if (typeof d.get !== 'function') throw new Error('compat-node was evaluated');
        for (const name of ['process', 'Buffer', 'setImmediate']) {{
          if (typeof Object.getOwnPropertyDescriptor(globalThis, name).get !== 'function') throw new Error(name + ' is not lazy');
        }}
        if (typeof op_buffer_encode !== 'undefined' || typeof op_node_bootstrap_source !== 'undefined') throw new Error('op leaked');
        "#
    );
    // `emit` runs the exit hooks too: they must not be what boots it.
    let (outcome, _) = run(Mode::Lazy, true, true, &body);
    assert_eq!(outcome, "ok");
}

#[test]
fn first_use_boots_it_and_leaves_the_same_globals_as_the_eager_install() {
    let body = format!(
        r#"
        const touch = [typeof process, typeof Buffer, typeof setImmediate];
        if (touch.join() !== 'object,function,function') throw new Error(touch.join());
        let ticked = false;
        process.nextTick(() => {{ ticked = true; }});
        await new Promise((resolve) => setImmediate(resolve));
        if (!ticked) throw new Error('nextTick drain did not run');
        if (Buffer.from('hi').toString('hex') !== '6869') throw new Error('Buffer');
        const names = ['Buffer', 'process', 'global', 'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'setImmediate', 'clearImmediate', {NODE_SYMBOL}];
        const shape = names.map((n) => {{
          const d = Object.getOwnPropertyDescriptor(globalThis, n);
          return [String(n), 'value' in d, !!d.writable, !!d.enumerable, !!d.configurable];
        }});
        const order = Reflect.ownKeys(globalThis).map(String).filter((k) => names.map(String).includes(k));
        report(JSON.stringify({{ shape, order }}));
        "#
    );
    let (lazy, lazy_report) = run(Mode::Lazy, false, false, &body);
    let (eager, eager_report) = run(Mode::Eager, false, false, &body);
    assert_eq!(
        (lazy.as_str(), eager.as_str()),
        ("ok", "ok"),
        "{lazy} / {eager}"
    );
    assert!(!lazy_report.is_empty());
    assert_eq!(lazy_report, eager_report);
}

#[test]
fn the_first_commonjs_lookup_boots_it_and_finds_the_builtins() {
    let body = r#"
        const hook = globalThis[Symbol.for('velyr.commonjs')];
        const before = Object.getOwnPropertyDescriptor(globalThis, Symbol.for('velyr.node'));
        if (typeof before.get !== 'function') throw new Error('booted too early');
        if (hook.builtins.has('fs') !== true) throw new Error('fs is not a builtin');
        if (typeof hook.builtins.get('path').join !== 'function') throw new Error('path');
        if (Object.getOwnPropertyNames(hook.builtins).includes('has')) throw new Error('patch left behind');
        if (Object.getOwnPropertyDescriptor(globalThis, Symbol.for('velyr.node')).get) throw new Error('not replaced');
    "#;
    let (outcome, _) = run(Mode::Lazy, true, false, body);
    assert_eq!(outcome, "ok");
}

#[test]
fn a_global_assigned_before_the_boot_keeps_the_scripts_value() {
    let body = r#"
        const mine = () => 'mine';
        globalThis.setTimeout = mine;
        globalThis.process = { fake: true };
        globalThis[Symbol.for('velyr.node')].require('path');
        if (globalThis.setTimeout !== mine) throw new Error('setTimeout was overwritten');
        if (!globalThis.process.fake) throw new Error('process was overwritten');
    "#;
    let (outcome, _) = run(Mode::Lazy, false, false, body);
    assert_eq!(outcome, "ok");
}

//! Node.js compatibility, Tier 1: `node:path`, `node:url`, `node:events`, `node:assert` and
//! `node:util` (plus `path/posix`, `path/win32`, `assert/strict`, `util/types`). ADR-007: opt-in.
//!
//! - [`install_compat_node`] defines the builtins in an isolate. If `velyr-compat-commonjs` was
//!   installed first, `require('node:x')` and `require('x')` find them.
//! - [`compat_node_host_functions`] are the byte ops `Buffer` needs; register them before
//!   `install_compat_node`. Without them everything but `Buffer` and `string_decoder` works.
//! - [`install_buffer_global`] opts into `globalThis.Buffer`.
//! - `process`, `global`, `setImmediate` and `node:process` / `node:timers`: register
//!   [`compat_node_host_functions`], call [`configure_process`] (optional) before
//!   [`install_compat_node`], then [`install_process_global`] from the `install` hook of
//!   `run_file`, and give the tick function it returns to `LoopIsolate::set_tick_hook`.
//!   `process.env` needs an installed `velyr_runtime_ops::OpContext` (permissions).
//! - [`NodeBuiltinResolver`] makes `import 'node:x'` / `import 'x'` resolve; give it to
//!   `RuntimeConfig::module_resolver` and `EsmRequire` (it wraps the resolver you already use).
//!
//! Errors carry the Node `code` (`ERR_INVALID_ARG_TYPE`, ...) on fresh JS errors. Kernel errors
//! (`VELYR_*`) that pass through these modules are never rewrapped, so their identity is kept.
//!
//! Known limits: see the notes at the top of each `src/*.js`.

mod async_context_ops;
mod buffer_ops;
mod child_process_ops;
mod crypto_cipher;
mod crypto_der;
mod crypto_gcm;
mod crypto_hash;
mod crypto_js;
mod crypto_keys;
mod crypto_ops;
mod crypto_sign;
mod crypto_stream;
mod dns_query;
mod fs_copy;
mod fs_jobs;
mod fs_js;
mod fs_ops;
mod http_client_ops;
mod http_ops;
mod http_upgrade_ops;
mod net_ops;
mod os_ops;
mod process_ops;
mod registry;
mod registry_async;
mod registry_extra;
mod registry_http;
mod registry_net;
mod registry_tables;
mod registry_test;
mod streams;
mod v8_ops;
mod worker_threads_ops;
mod zlib_codec;
mod zlib_deflate;
mod zlib_gzip;
mod zlib_ops;
mod zlib_zstd;

use velyr_engine_api::{Isolate, JsException, Source, Value};
use velyr_runtime_event_loop::TimerHandle;

pub use buffer_ops::compat_node_host_functions;
pub use process_ops::ProcessConfig;
pub use registry::{BUILTINS, Builtin, NodeBuiltinResolver, builtin_name, facade_source};

/// Defines the Tier 1 builtins in `isolate`. Call after `install_commonjs`, if that is used.
pub fn install_compat_node(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate.set_slot(http_client_ops::HttpState::default());
    isolate
        .eval_script(&Source {
            name: "velyr:compat-node".into(),
            text: registry::bootstrap_source(),
        })
        .map(drop)
}

/// Defines `globalThis.Buffer` (non-enumerable, like Node). Opt-in: hosts call it for the node
/// and full profiles only. Needs [`install_compat_node`] first.
pub fn install_buffer_global(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate
        .eval_script(&Source {
            name: "velyr:compat-node-buffer-global".into(),
            text: r#"Object.defineProperty(globalThis, "Buffer", {
                value: globalThis[Symbol.for("velyr.node")].require("buffer").Buffer,
                writable: true, configurable: true, enumerable: false });"#
                .into(),
        })
        .map(drop)
}

/// Sets what `process.argv`, `process.execPath` and `process.exit` report. Call before
/// [`install_compat_node`] (the `process` module reads it when it loads).
pub fn configure_process(isolate: &mut impl Isolate, config: ProcessConfig) {
    isolate.set_slot(config);
}

fn eval_value(isolate: &mut impl Isolate, name: &str, text: &str) -> Result<Value, JsException> {
    isolate.eval_script(&Source {
        name: name.into(),
        text: text.into(),
    })
}

/// Defines `process` (non-enumerable), `global`, `setImmediate` and `clearImmediate` on the
/// global object and hooks `setImmediate` to the loop's check phase. Needs [`install_compat_node`]
/// first. Returns the `nextTick` drain function: pass it to `LoopIsolate::set_tick_hook` so ticks
/// run before promise microtasks.
pub fn install_process_global(
    isolate: &mut impl Isolate,
    timers: TimerHandle,
) -> Result<Value, JsException> {
    let immediates = timers.immediates();
    isolate.set_slot(process_ops::ImmediateSlot(immediates.clone()));
    let drain = eval_value(
        isolate,
        "velyr:compat-node-process-global",
        r#"(function () {
            const node = globalThis[Symbol.for("velyr.node")];
            const process = node.require("process");
            const timers = node.require("timers");
            const define = (name, value, enumerable) => Object.defineProperty(globalThis, name,
                { value, writable: true, configurable: true, enumerable });
            define("process", process, false);
            define("global", globalThis, true);
            for (const name of ["setTimeout", "clearTimeout", "setInterval", "clearInterval"]) {
                define(name, timers[name], true);
            }
            define("setImmediate", timers.setImmediate, true);
            define("clearImmediate", timers.clearImmediate, true);
            return process._tickCallback;
        })()"#,
    )?;
    let dispatch = eval_value(
        isolate,
        "velyr:compat-node-immediate-dispatch",
        r#"globalThis[Symbol.for("velyr.node")].require("timers")[Symbol.for("velyr.immediate.dispatch")]"#,
    )?;
    immediates.set_dispatcher(dispatch);
    Ok(drain)
}

fn emit(isolate: &mut impl Isolate, args: &str, values: &[Value]) -> Result<Value, JsException> {
    let function = eval_value(
        isolate,
        "velyr:compat-node-emit",
        &format!(
            r#"(function (a, b) {{ return globalThis[Symbol.for("velyr.node")].require("process").emit({args}); }})"#
        ),
    )?;
    isolate.call(&function, &Value::Undefined, values)
}

/// Emits `process.on('beforeExit')`; the host re-checks the loop afterwards and may call it
/// again once the loop drains.
pub fn emit_before_exit(isolate: &mut impl Isolate, code: i32) -> Result<(), JsException> {
    emit(isolate, "'beforeExit', a", &[Value::Number(code.into())]).map(drop)
}

/// Emits `process.on('exit')`.
pub fn emit_exit(isolate: &mut impl Isolate, code: i32) -> Result<(), JsException> {
    emit(isolate, "'exit', a", &[Value::Number(code.into())]).map(drop)
}

/// Emits `process.on('unhandledRejection', reason)`; `true` when a listener handled it.
pub fn emit_unhandled_rejection(
    isolate: &mut impl Isolate,
    reason: &Value,
) -> Result<bool, JsException> {
    let handled = emit(
        isolate,
        "'unhandledRejection', a, undefined",
        std::slice::from_ref(reason),
    )?;
    Ok(handled == Value::Bool(true))
}

/// Emits a signal event (`SIGINT`, `SIGTERM`); `true` when a listener exists.
pub fn emit_signal(isolate: &mut impl Isolate, signal: &str) -> Result<bool, JsException> {
    let handled = emit(isolate, "a, a", &[Value::String(signal.into())])?;
    Ok(handled == Value::Bool(true))
}

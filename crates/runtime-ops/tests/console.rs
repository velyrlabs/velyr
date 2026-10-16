//! Runs the embedded bootstrap on V8 with a capturing `op_console_write`.
use std::cell::RefCell;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{Runtime, RuntimeConfig, RuntimeIsolate};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry};

thread_local! {
    static LINES: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

fn capture(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(level), Value::String(text)] = args else {
        return Err(HostError::TypeError("expected (level, text)".into()));
    };
    LINES.with(|l| {
        l.borrow_mut()
            .push((level.to_string_lossy(), text.to_string_lossy()))
    });
    Ok(Value::Undefined)
}

fn isolate() -> RuntimeIsolate<V8Engine> {
    LINES.with(|l| l.borrow_mut().clear());
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        ..RuntimeConfig::default()
    };
    let mut isolate = Runtime::<V8Engine>::new(config).create_isolate().unwrap();
    let mut registry = OpRegistry::new();
    registry
        .register(HostFunctionDef {
            name: "op_console_write",
            arity: 2,
            callback: capture,
        })
        .unwrap();
    isolate
        .bootstrap(registry.host_functions(), BOOTSTRAP_JS)
        .unwrap();
    isolate
}

fn logged(code: &str) -> Vec<(String, String)> {
    let mut isolate = isolate();
    isolate.eval(code).unwrap();
    LINES.with(|l| l.borrow().clone())
}

fn line(code: &str) -> String {
    logged(code).remove(0).1
}

#[test]
fn levels_are_forwarded() {
    let lines = logged(
        "console.log('a'); console.info('b'); console.debug('c'); console.warn('d'); console.error('e')",
    );
    let levels: Vec<_> = lines.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(levels, ["log", "info", "debug", "warn", "error"]);
}

#[test]
fn format_specifiers() {
    assert_eq!(line("console.log('%s is %d%%', 'x', 5.5)"), "x is 5.5%");
    assert_eq!(line("console.log('%o', {a: 1})"), "{ a: 1 }");
    assert_eq!(line("console.log('%s')"), "%s");
}

#[test]
fn extra_args_are_appended() {
    assert_eq!(
        line("console.log('a', 'b', 1, null, undefined)"),
        "a b 1 null undefined"
    );
    assert_eq!(line("console.log(1, 'x')"), "1 x");
    assert_eq!(line("console.log()"), "");
}

#[test]
fn objects_are_inspected() {
    assert_eq!(
        line("console.log({a: 1, b: 'x', c: [1, 2], d: {e: null}})"),
        "{ a: 1, b: 'x', c: [ 1, 2 ], d: { e: null } }"
    );
    assert_eq!(line("console.log([], {})"), "[] {}");
    assert_eq!(
        line("console.log(function foo() {}, 10n, Symbol('s'))"),
        "[Function: foo] 10n Symbol(s)"
    );
}

#[test]
fn cycles_and_depth_are_bounded() {
    assert_eq!(
        line("const o = {}; o.self = o; console.log(o)"),
        "{ self: [Circular] }"
    );
    assert_eq!(
        line("console.log({a: {b: {c: {d: 1}}}})"),
        "{ a: { b: { c: [Object] } } }"
    );
}

#[test]
fn errors_print_their_stack() {
    assert!(line("console.log(new Error('boom'))").starts_with("Error: boom"));
}

#[test]
fn raw_op_is_removed_from_global() {
    let mut isolate = isolate();
    let leaked = isolate
        .eval("Object.getOwnPropertyNames(globalThis).some((n) => n.startsWith('op_'))")
        .unwrap();
    assert_eq!(leaked, Value::Bool(false));
    let console = isolate.eval("typeof console.log").unwrap();
    assert!(matches!(console, Value::String(_)));
}

#[test]
fn console_keeps_working_after_user_tampering() {
    assert_eq!(
        line("globalThis.op_console_write = null; console.log('ok')"),
        "ok"
    );
}

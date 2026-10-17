//! Security pins for module sources handed to V8 as external one-byte strings (ML-12, audited by
//! ML-19 item 7): the `Arc<str>` keepalive outlives every V8 reference and is released exactly
//! once, only ASCII takes the external path, and over-long text is an error, not a panic.

use std::sync::{Arc, Weak};

use velyr_engine_api::{
    Engine, GcReason, HostError, Isolate, IsolateOptions, JsString, ModuleResolveHook,
    SharedSource, Source, Value,
};
use velyr_engine_v8::{V8Engine, V8Isolate};
use velyr_runtime_errors::VelyrError;

fn isolate() -> V8Isolate {
    V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn shared(name: &str, text: &Arc<str>) -> SharedSource {
    SharedSource {
        name: name.into(),
        text: Arc::clone(text),
    }
}

fn eval(iso: &mut V8Isolate, expr: &str) -> Value {
    let probe = Source {
        name: "probe.js".into(),
        text: expr.into(),
    };
    iso.eval_script(&probe).expect("probe")
}

struct NoImports;

impl ModuleResolveHook for NoImports {
    fn resolve(&mut self, specifier: &str, _: &str) -> Result<Source, HostError> {
        Err(HostError::Velyr(VelyrError::not_found(
            specifier.to_owned(),
        )))
    }
}

/// Compiles, links and evaluates `text` from a shared buffer.
fn run_shared(iso: &mut V8Isolate, name: &str, text: &Arc<str>) {
    let module = iso
        .compile_module_shared(&shared(name, text), None)
        .expect("compile shared");
    iso.instantiate_module(module, &mut NoImports)
        .expect("instantiate");
    iso.evaluate_module(module).expect("evaluate");
    iso.perform_microtask_checkpoint();
}

fn string(text: &str) -> Value {
    Value::String(JsString::from(text))
}

// The source string is read again after the caller let go: lazy compilation of `inner` and
// `Function.prototype.toString` both slice the external buffer.
#[test]
fn external_string_outlives_the_graph() {
    let inner = "function inner(x) { return x + 1; }";
    let text: Arc<str> = Arc::from(format!(
        "{inner}\nglobalThis.inner = inner; globalThis.ok = true;"
    ));
    let weak: Weak<str> = Arc::downgrade(&text);
    let mut iso = isolate();
    run_shared(&mut iso, "outlive.js", &text);
    drop(text);
    for _ in 0..3 {
        iso.request_gc(GcReason::Explicit);
    }
    assert!(weak.upgrade().is_some(), "V8 still references the buffer");
    assert_eq!(eval(&mut iso, "inner(41)"), Value::Number(42.0));
    assert_eq!(eval(&mut iso, "inner.toString()"), string(inner));
    drop(iso);
    assert!(weak.upgrade().is_none(), "teardown releases the keepalive");
}

#[test]
fn each_external_string_releases_exactly_one_clone() {
    let text: Arc<str> = Arc::from("globalThis.n = (globalThis.n ?? 0) + 1;");
    let mut iso = isolate();
    for name in ["one.js", "two.js", "three.js"] {
        run_shared(&mut iso, name, &text);
    }
    assert_eq!(Arc::strong_count(&text), 4);
    assert_eq!(eval(&mut iso, "n"), Value::Number(3.0));
    drop(iso);
    assert_eq!(Arc::strong_count(&text), 1);
}

#[test]
fn keepalive_released_by_an_isolate_on_another_thread() {
    let text: Arc<str> = Arc::from("globalThis.t = 7;");
    let moved = Arc::clone(&text);
    std::thread::spawn(move || {
        let mut iso = isolate();
        run_shared(&mut iso, "thread.js", &moved);
        assert_eq!(eval(&mut iso, "t"), Value::Number(7.0));
    })
    .join()
    .expect("worker thread");
    assert_eq!(Arc::strong_count(&text), 1);
}

#[test]
fn non_ascii_source_is_copied() {
    let cases = [
        ("latin1.js", "globalThis.s = 'ação';", "ação"),
        (
            "utf8.js",
            "globalThis.s = '\u{1f600}\u{4e2d}';",
            "\u{1f600}\u{4e2d}",
        ),
        ("tail.js", "globalThis.s = 'abc'; // \u{e9}", "abc"),
    ];
    for (name, text, expected) in cases {
        let text: Arc<str> = Arc::from(text);
        let mut iso = isolate();
        let module = iso
            .compile_module_shared(&shared(name, &text), None)
            .expect("compile");
        assert_eq!(Arc::strong_count(&text), 1, "{name} took the external path");
        iso.instantiate_module(module, &mut NoImports)
            .expect("instantiate");
        iso.evaluate_module(module).expect("evaluate");
        assert_eq!(eval(&mut iso, "s"), string(expected), "{name}");
    }
}

#[test]
fn empty_source_is_copied() {
    let text: Arc<str> = Arc::from("");
    let mut iso = isolate();
    iso.compile_module_shared(&shared("empty.js", &text), None)
        .expect("compile");
    assert_eq!(Arc::strong_count(&text), 1);
}

// Allocates about 0.5 GiB once; V8 refuses the length before copying anything.
#[test]
fn over_long_source_is_an_error_not_a_panic() {
    let text: Arc<str> = Arc::from("a".repeat(v8::String::MAX_LENGTH + 1));
    let mut iso = isolate();
    let err = iso
        .compile_module_shared(&shared("huge.js", &text), None)
        .expect_err("over-long source");
    assert!(err.message.contains("too long"), "{}", err.message);
    assert_eq!(Arc::strong_count(&text), 1, "no keepalive leaked");
    drop(text);
    assert_eq!(eval(&mut iso, "1 + 1"), Value::Number(2.0));
}

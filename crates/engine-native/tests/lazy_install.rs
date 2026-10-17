//! Intl and Temporal install lazily per isolate (N06): the globals are shells until first
//! touched, and the locale-sensitive built-ins force `Intl` themselves.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, text: &str) -> String {
    let source = Source {
        name: "lazy_install.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(Value::String(s)) => s.to_string_lossy(),
        Ok(Value::Number(n)) => n.to_string(),
        Ok(other) => format!("{other:?}"),
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

#[test]
fn both_namespaces_are_pending_after_creation() {
    assert_eq!(isolate().pending_lazy_globals(), ["Intl", "Temporal"]);
}

#[test]
fn typeof_and_descriptor_do_not_materialize() {
    let mut iso = isolate();
    assert_eq!(
        eval(&mut iso, "typeof Intl + typeof Temporal"),
        "objectobject"
    );
    let descriptor = eval(
        &mut iso,
        "JSON.stringify(Object.getOwnPropertyDescriptor(globalThis, 'Intl'), ['writable','enumerable','configurable'])",
    );
    assert_eq!(
        descriptor,
        r#"{"writable":true,"enumerable":false,"configurable":true}"#
    );
    assert_eq!(iso.pending_lazy_globals(), ["Intl", "Temporal"]);
}

#[test]
fn locale_sensitive_builtins_force_intl_only() {
    let mut iso = isolate();
    assert_eq!(
        eval(&mut iso, "Number.prototype.toLocaleString.length"),
        "0"
    );
    assert_eq!(iso.pending_lazy_globals(), ["Intl", "Temporal"]);
    assert_eq!(eval(&mut iso, "(1234.5).toLocaleString('en')"), "1,234.5");
    assert_eq!(iso.pending_lazy_globals(), ["Temporal"]);
    assert_eq!(
        eval(&mut iso, "Number.prototype.toLocaleString.length"),
        "0"
    );
    assert_eq!(eval(&mut iso, "String.prototype.localeCompare.length"), "1");
}

#[test]
fn stub_methods_hide_their_source() {
    let mut iso = isolate();
    let text = eval(&mut iso, "Date.prototype.toLocaleString.toString()");
    assert!(text.contains("native code"), "{text}");
}

#[test]
fn intl_and_temporal_work_after_materialization() {
    let mut iso = isolate();
    assert_eq!(
        eval(&mut iso, "new Intl.NumberFormat('en').format(1234.5)"),
        "1,234.5"
    );
    assert_eq!(
        eval(&mut iso, "Temporal.PlainDate.from('2024-01-02').year"),
        "2024"
    );
    assert_eq!(
        eval(&mut iso, "Object.prototype.toString.call(Temporal)"),
        "[object Temporal]"
    );
    assert_eq!(
        eval(
            &mut iso,
            "Temporal.PlainDate.prototype.add.toString().includes('native code')"
        ),
        "Bool(true)"
    );
    assert!(iso.pending_lazy_globals().is_empty());
}

#[cfg(feature = "intl-all-locales")]
#[test]
fn german_array_to_locale_string() {
    assert_eq!(
        eval(&mut isolate(), "[1234.5].toLocaleString('de')"),
        "1.234,5"
    );
}

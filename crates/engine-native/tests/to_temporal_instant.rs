//! `Date.prototype.toTemporalInstant` is eager and tiny: it must not materialize `Temporal`
//! until it is called.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, text: &str) -> String {
    let source = Source {
        name: "to_temporal_instant.js".into(),
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
fn converts_to_an_instant() {
    let mut iso = isolate();
    assert_eq!(
        eval(
            &mut iso,
            "new Date(1700000000000).toTemporalInstant().epochMilliseconds"
        ),
        "1700000000000"
    );
}

#[test]
fn has_method_shape() {
    let mut iso = isolate();
    let shape = "const f = Date.prototype.toTemporalInstant; const d = Object.getOwnPropertyDescriptor(Date.prototype, 'toTemporalInstant'); \
        [f.length, f.name, d.writable, d.enumerable, d.configurable, f.toString().includes('[native code]')].join()";
    assert_eq!(
        eval(&mut iso, shape),
        "0,toTemporalInstant,true,false,true,true"
    );
}

#[test]
fn invalid_date_throws_range_error() {
    let mut iso = isolate();
    assert_eq!(
        eval(
            &mut iso,
            "try { new Date(NaN).toTemporalInstant() } catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
}

#[test]
fn non_date_receiver_throws_type_error() {
    let mut iso = isolate();
    assert_eq!(
        eval(
            &mut iso,
            "try { Date.prototype.toTemporalInstant.call({}) } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn does_not_leak_the_force_global() {
    let mut iso = isolate();
    assert_eq!(
        eval(&mut iso, "typeof globalThis.__lazy_force"),
        "undefined"
    );
}

#[test]
fn temporal_stays_pending_until_the_first_call() {
    let mut iso = isolate();
    assert_eq!(iso.pending_lazy_globals(), ["Intl", "Temporal"]);
    eval(
        &mut iso,
        "new Date(0).toTemporalInstant().epochMilliseconds",
    );
    assert_eq!(iso.pending_lazy_globals(), ["Intl"]);
}

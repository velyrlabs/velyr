//! Shared Temporal helpers (N22): option coercion, method descriptors, calendar id validation
//! and spec-order option reads.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal_helpers.js".into(),
        text: code.into(),
    };
    match iso.eval_script(&source) {
        Ok(value) => value,
        Err(error) => panic!("`{code}` threw {}", error.message),
    }
}

/// Evaluates each `(code, expected string)` pair in one isolate.
fn expect_strings(cases: &[(&str, &str)]) {
    let mut iso = isolate();
    for (code, expected) in cases {
        let expected = Value::String(JsString::from(*expected));
        assert_eq!(eval(&mut iso, code), expected, "{code}");
    }
}

/// `code` must throw an instance of the error constructor `kind`.
fn expect_throws(kind: &str, cases: &[&str]) {
    let mut iso = isolate();
    for code in cases {
        let probe = format!(
            "(() => {{ try {{ {code}; return 'no throw'; }} catch (e) {{ return e instanceof {kind} ? 'ok' : String(e); }} }})()"
        );
        assert_eq!(
            eval(&mut iso, &probe),
            Value::String(JsString::from("ok")),
            "{code}"
        );
    }
}

#[test]
fn function_options_are_accepted() {
    expect_strings(&[(
        "String(Temporal.Duration.compare({years: 1}, {years: 1}, function () {}))",
        "0",
    )]);
}

#[test]
fn symbol_option_values_throw_type_error() {
    expect_throws(
        "TypeError",
        &[
            "Temporal.Duration.from({days: 1}).round({smallestUnit: Symbol()})",
            "Temporal.PlainDate.from('2024-01-01').add({days: 1}, {overflow: Symbol()})",
        ],
    );
}

#[test]
fn method_lengths_follow_the_spec() {
    expect_strings(&[
        ("String(Temporal.PlainDate.prototype.add.length)", "1"),
        ("String(Temporal.PlainDate.prototype.until.length)", "1"),
        ("Temporal.PlainDate.prototype.add.name", "add"),
        (
            "String(Object.getOwnPropertyDescriptor(Temporal.PlainDate.prototype, 'add').enumerable)",
            "false",
        ),
        (
            "String(Temporal.PlainDate.prototype.add.hasOwnProperty('prototype'))",
            "false",
        ),
        (
            "(() => { try { new Temporal.PlainDate.prototype.add(); return 'no throw'; } catch (e) { return e.constructor.name; } })()",
            "TypeError",
        ),
    ]);
}

#[test]
fn calendar_id_is_canonicalized_and_validated() {
    expect_strings(&[(
        "Temporal.PlainDate.from('2024-01-01[u-ca=ISO8601]').calendarId",
        "iso8601",
    )]);
    expect_throws(
        "RangeError",
        &["Temporal.PlainDate.from('2024-01-01[u-ca=]')"],
    );
}

#[test]
fn difference_options_are_read_in_spec_order() {
    expect_strings(&[(
        "(() => { const log = []; const proxy = new Proxy({}, { get(t, k) { log.push(String(k)); return undefined; } }); \
         Temporal.PlainDate.from('2024-01-01').until('2024-02-01', proxy); return log.join(','); })()",
        "largestUnit,roundingIncrement,roundingMode,smallestUnit",
    )]);
}

#[test]
fn round_options_are_read_once_in_spec_order() {
    expect_strings(&[(
        "(() => { const log = []; const proxy = new Proxy({ smallestUnit: 'day' }, { get(t, k) { log.push(String(k)); return t[k]; } }); \
         Temporal.PlainDateTime.from('2024-01-01T10:00').round(proxy); return log.join(','); })()",
        "roundingIncrement,roundingMode,smallestUnit",
    )]);
}

#[test]
fn string_options_are_read_in_spec_order() {
    expect_strings(&[(
        "(() => { const log = []; const proxy = new Proxy({}, { get(t, k) { log.push(String(k)); return undefined; } }); \
         Temporal.PlainTime.from('10:00').toString(proxy); return log.join(','); })()",
        "fractionalSecondDigits,roundingMode,smallestUnit",
    )]);
}

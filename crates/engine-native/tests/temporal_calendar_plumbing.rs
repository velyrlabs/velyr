//! Temporal calendar plumbing (N45): any calendar `temporal_rs` supports is accepted and
//! carried in the `[u-ca=...]` annotation of the canonical string.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal_calendar_plumbing.js".into(),
        text: code.into(),
    };
    match iso.eval_script(&source) {
        Ok(value) => value,
        Err(error) => panic!("`{code}` threw {}", error.message),
    }
}

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
fn plain_date_with_calendar() {
    expect_strings(&[
        (
            "Temporal.PlainDate.from('2024-03-15').withCalendar('gregory').calendarId",
            "gregory",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15').withCalendar('gregory').toString()",
            "2024-03-15[u-ca=gregory]",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15').withCalendar('ISO8601').toString()",
            "2024-03-15",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15').withCalendar('hebrew').calendarId",
            "hebrew",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15').withCalendar('ISLAMIC-CIVIL').calendarId",
            "islamic-civil",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15').withCalendar('islamicc').calendarId",
            "islamic-civil",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15').withCalendar('ethiopic-amete-alem').calendarId",
            "ethioaa",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').calendarId",
            "gregory",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15').withCalendar('2020-01-01[u-ca=gregory]').calendarId",
            "gregory",
        ),
        (
            "new Temporal.PlainDate(2024, 3, 15, 'gregory').toString()",
            "2024-03-15[u-ca=gregory]",
        ),
    ]);
}

#[test]
fn calendar_name_option_on_non_iso() {
    expect_strings(&[
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').toString({calendarName:'never'})",
            "2024-03-15",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').toString({calendarName:'critical'})",
            "2024-03-15[!u-ca=gregory]",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').toString({calendarName:'always'})",
            "2024-03-15[u-ca=gregory]",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15').toString({calendarName:'always'})",
            "2024-03-15[u-ca=iso8601]",
        ),
    ]);
}

#[test]
fn plain_date_time_with_calendar() {
    expect_strings(&[
        (
            "Temporal.PlainDateTime.from('2024-03-15T10:30:00').withCalendar('gregory').toString()",
            "2024-03-15T10:30:00[u-ca=gregory]",
        ),
        (
            "Temporal.PlainDateTime.from('2024-03-15T10:30:00[u-ca=hebrew]').calendarId",
            "hebrew",
        ),
        (
            "Temporal.PlainDateTime.from('2024-03-15T10:30:00[u-ca=gregory]').withCalendar('iso8601').toString()",
            "2024-03-15T10:30:00",
        ),
        (
            "Temporal.PlainDateTime.from('2024-03-15T10:30:00[u-ca=gregory]').withPlainTime('01:02').toString()",
            "2024-03-15T01:02:00[u-ca=gregory]",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').toPlainDateTime('01:02').toString()",
            "2024-03-15T01:02:00[u-ca=gregory]",
        ),
    ]);
}

#[test]
fn zoned_date_time_with_calendar() {
    expect_strings(&[
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC]').withCalendar('gregory').calendarId",
            "gregory",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC]').withCalendar('gregory').toString()",
            "2024-03-15T10:00:00+00:00[UTC][u-ca=gregory]",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=hebrew]').calendarId",
            "hebrew",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=gregory]').withCalendar('iso8601').toString()",
            "2024-03-15T10:00:00+00:00[UTC]",
        ),
        (
            "new Temporal.ZonedDateTime(0n, 'UTC', 'gregory').toString()",
            "1970-01-01T00:00:00+00:00[UTC][u-ca=gregory]",
        ),
    ]);
}

#[test]
fn rejects_unknown_calendars() {
    expect_throws(
        "RangeError",
        &[
            "Temporal.PlainDate.from('2024-03-15').withCalendar('nonsense')",
            "Temporal.PlainDateTime.from('2024-03-15T10:00').withCalendar('nonsense')",
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC]').withCalendar('nonsense')",
            "new Temporal.PlainDate(2024, 3, 15, 'nonsense')",
        ],
    );
    expect_throws(
        "TypeError",
        &["Temporal.PlainDate.from('2024-03-15').withCalendar(1)"],
    );
}

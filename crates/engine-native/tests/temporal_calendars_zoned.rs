//! Non-ISO calendar fields and arithmetic for Instant and ZonedDateTime (N47).

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal_calendars_zoned.js".into(),
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
fn zoned_fields() {
    expect_strings(&[
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=hebrew]').monthCode",
            "M06",
        ),
        (
            "String(Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=hebrew]').year)",
            "5784",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=gregory]').era",
            "ce",
        ),
        (
            "String(Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=japanese]').eraYear)",
            "6",
        ),
        (
            "String(Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=gregory]').weekOfYear)",
            "undefined",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC]').withCalendar('gregory').calendarId",
            "gregory",
        ),
    ]);
}

#[test]
fn zoned_arithmetic_keeps_calendar() {
    expect_strings(&[
        (
            "Temporal.ZonedDateTime.from('2024-01-31T10:00:00+00:00[UTC][u-ca=gregory]').add({months:1}).toString()",
            "2024-02-29T10:00:00+00:00[UTC][u-ca=gregory]",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=hebrew]').add({months:1}).toString()",
            "2024-04-13T10:00:00+00:00[UTC][u-ca=hebrew]",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=gregory]').toPlainDate().toString()",
            "2024-03-15[u-ca=gregory]",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=gregory]').equals('2024-03-15T10:00:00+00:00[UTC]') + ''",
            "false",
        ),
    ]);
}

#[test]
fn zoned_bags() {
    expect_strings(&[
        (
            "Temporal.ZonedDateTime.from({calendar:'japanese',era:'reiwa',eraYear:6,month:3,day:15,timeZone:'UTC'}).toString()",
            "2024-03-15T00:00:00+00:00[UTC][u-ca=japanese]",
        ),
        (
            "Temporal.ZonedDateTime.from({calendar:'hebrew',year:5784,monthCode:'M06',day:5,hour:3,timeZone:'UTC'}).toString()",
            "2024-03-15T03:00:00+00:00[UTC][u-ca=hebrew]",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=gregory]').with({era:'ce',eraYear:2023}).toString()",
            "2023-03-15T10:00:00+00:00[UTC][u-ca=gregory]",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC][u-ca=hebrew]').with({day:1}).toString()",
            "2024-03-11T10:00:00+00:00[UTC][u-ca=hebrew]",
        ),
    ]);
}

#[test]
fn instant_ignores_calendar_annotation() {
    expect_strings(&[(
        "Temporal.Instant.from('2024-03-15T10:00:00Z[u-ca=gregory]').toString()",
        "2024-03-15T10:00:00Z",
    )]);
}

#[test]
fn rejections() {
    expect_throws(
        "RangeError",
        &[
            "Temporal.ZonedDateTime.from({calendar:'gregory',year:2024,monthCode:'M13',day:1,timeZone:'UTC'},{overflow:'reject'})",
            "Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC]').withCalendar('islamic')",
        ],
    );
}

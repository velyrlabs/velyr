//! Non-ISO calendar fields and arithmetic for PlainDate and PlainDateTime (N46).

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal_calendars_plain.js".into(),
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
fn plain_date_fields() {
    expect_strings(&[
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=hebrew]').monthCode",
            "M06",
        ),
        (
            "String(Temporal.PlainDate.from('2024-03-15[u-ca=hebrew]').year)",
            "5784",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').era",
            "ce",
        ),
        (
            "String(Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').eraYear)",
            "2024",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=japanese]').era",
            "reiwa",
        ),
        (
            "String(Temporal.PlainDate.from('2024-03-15[u-ca=japanese]').eraYear)",
            "6",
        ),
        (
            "String(Temporal.PlainDate.from('2024-03-15').era)",
            "undefined",
        ),
        (
            "String(Temporal.PlainDate.from('2024-03-15[u-ca=chinese]').era)",
            "undefined",
        ),
        (
            "String(Temporal.PlainDate.from('2024-03-15[u-ca=hebrew]').monthsInYear)",
            "13",
        ),
        (
            "String(Temporal.PlainDate.from('2024-03-15[u-ca=hebrew]').inLeapYear)",
            "true",
        ),
        (
            "String(Temporal.PlainDateTime.from('2024-03-15T10:00[u-ca=hebrew]').year)",
            "5784",
        ),
        (
            "Temporal.PlainDateTime.from('2024-03-15T10:00[u-ca=gregory]').era",
            "ce",
        ),
    ]);
}

#[test]
fn plain_date_from_bags() {
    expect_strings(&[
        (
            "Temporal.PlainDate.from({calendar:'japanese',era:'reiwa',eraYear:6,month:3,day:15}).toString()",
            "2024-03-15[u-ca=japanese]",
        ),
        (
            "Temporal.PlainDate.from({calendar:'hebrew',year:5784,monthCode:'M06',day:5}).toString()",
            "2024-03-15[u-ca=hebrew]",
        ),
        (
            "Temporal.PlainDateTime.from({calendar:'hebrew',year:5784,monthCode:'M06',day:5,hour:10}).toString()",
            "2024-03-15T10:00:00[u-ca=hebrew]",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=hebrew]').with({day:6}).toString()",
            "2024-03-16[u-ca=hebrew]",
        ),
        (
            "Temporal.PlainDateTime.from('2024-03-15T10:00[u-ca=hebrew]').with({day:6}).toString()",
            "2024-03-16T10:00:00[u-ca=hebrew]",
        ),
    ]);
}

#[test]
fn plain_date_arithmetic() {
    expect_strings(&[
        (
            "Temporal.PlainDate.from('2024-01-31[u-ca=gregory]').add({months:1}).toString()",
            "2024-02-29[u-ca=gregory]",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=hebrew]').add({months:1}).toString()",
            "2024-04-13[u-ca=hebrew]",
        ),
        (
            "String(Temporal.PlainDate.from('2024-03-15[u-ca=islamic-umalqura]').until('2024-09-15[u-ca=islamic-umalqura]',{largestUnit:'months'}).months)",
            "6",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').equals('2024-03-15[u-ca=gregory]') + ''",
            "true",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').equals('2024-03-15') + ''",
            "false",
        ),
        (
            "Temporal.PlainDateTime.from('2024-01-31T10:00[u-ca=gregory]').add({months:1}).toString()",
            "2024-02-29T10:00:00[u-ca=gregory]",
        ),
    ]);
}

#[test]
fn rejects_mismatched_calendars() {
    expect_throws(
        "RangeError",
        &[
            "Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').until('2024-03-15')",
            "Temporal.PlainDateTime.from('2024-03-15T10:00[u-ca=gregory]').since('2024-03-15T10:00')",
            "Temporal.PlainDate.from({calendar:'gregory',era:'nonsense',eraYear:1,month:1,day:1})",
            "Temporal.PlainDate.from({calendar:'hebrew',year:5783,monthCode:'M05L',day:1},{overflow:'reject'})",
        ],
    );
    expect_throws(
        "TypeError",
        &["Temporal.PlainDate.from({calendar:'gregory',era:'ce',month:1,day:1})"],
    );
}

#[test]
fn islamic_is_not_a_temporal_calendar() {
    expect_throws(
        "RangeError",
        &[
            "Temporal.PlainDate.from({year:1500,month:1,day:1,calendar:'islamic'})",
            "Temporal.PlainDate.from('2024-03-15').withCalendar('Islamic')",
        ],
    );
    expect_strings(&[(
        "Temporal.PlainDate.from('2024-03-15').withCalendar('T11:30[u-ca=hebrew]').calendarId",
        "hebrew",
    )]);
}

#[test]
fn week_numbers_exist_only_in_iso() {
    expect_strings(&[
        (
            "String(Temporal.PlainDate.from('2024-03-15').weekOfYear)",
            "11",
        ),
        (
            "String(Temporal.PlainDate.from('2024-03-15[u-ca=gregory]').weekOfYear)",
            "undefined",
        ),
        (
            "String(Temporal.PlainDateTime.from('2024-03-15T10:00[u-ca=hebrew]').yearOfWeek)",
            "undefined",
        ),
    ]);
}

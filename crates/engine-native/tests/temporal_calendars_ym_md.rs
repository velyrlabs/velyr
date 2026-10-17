//! Non-ISO calendars for PlainYearMonth and PlainMonthDay (N48).

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal_calendars_ym_md.js".into(),
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
fn year_month_hebrew_leap_months() {
    expect_strings(&[
        (
            "Temporal.PlainYearMonth.from({calendar:'hebrew',year:5784,monthCode:'M05L'}).monthCode",
            "M05L",
        ),
        (
            "Temporal.PlainYearMonth.from({calendar:'hebrew',year:5783,monthCode:'M05L'}).monthCode",
            "M06",
        ),
        (
            "Temporal.PlainYearMonth.from({calendar:'hebrew',year:5784,month:6}).monthCode",
            "M05L",
        ),
        (
            "Temporal.PlainYearMonth.from({calendar:'hebrew',year:5784,month:7}).monthCode",
            "M06",
        ),
        (
            "String(Temporal.PlainYearMonth.from({calendar:'hebrew',year:5784,month:6}).monthsInYear)",
            "13",
        ),
    ]);
    expect_throws(
        "RangeError",
        &[
            "Temporal.PlainYearMonth.from({calendar:'hebrew',year:5783,monthCode:'M05L'},{overflow:'reject'})",
        ],
    );
}

#[test]
fn year_month_fields_and_strings() {
    expect_strings(&[
        (
            "Temporal.PlainYearMonth.from('2024-03-01[u-ca=gregory]').era",
            "ce",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03-01[u-ca=gregory]').toString()",
            "2024-03-01[u-ca=gregory]",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03-01[u-ca=gregory]').calendarId",
            "gregory",
        ),
        (
            "String(Temporal.PlainYearMonth.from('2024-03-01[u-ca=gregory]').eraYear)",
            "2024",
        ),
        (
            "String(Temporal.PlainYearMonth.from('2024-03-01[u-ca=hebrew]').year)",
            "5784",
        ),
        (
            "String(new Temporal.PlainYearMonth(2024, 3, 'gregory', 1).daysInMonth)",
            "31",
        ),
        (
            "Temporal.PlainYearMonth.from({calendar:'japanese',era:'reiwa',eraYear:6,month:3}).toString()",
            "2024-03-01[u-ca=japanese]",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03-01[u-ca=gregory]').toString({calendarName:'never'})",
            "2024-03-01",
        ),
        (
            "String(Temporal.PlainYearMonth.from('2024-03').era)",
            "undefined",
        ),
    ]);
}

#[test]
fn year_month_arithmetic_and_conversion() {
    expect_strings(&[
        (
            "Temporal.PlainYearMonth.from('2024-03-01[u-ca=gregory]').add({months:1}).toString()",
            "2024-04-01[u-ca=gregory]",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03-01[u-ca=gregory]').with({month:5}).toString()",
            "2024-05-01[u-ca=gregory]",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03-01[u-ca=gregory]').toPlainDate({day:15}).toString()",
            "2024-03-15[u-ca=gregory]",
        ),
        (
            "String(Temporal.PlainYearMonth.from('2024-03-01[u-ca=gregory]').equals('2024-03-01[u-ca=gregory]'))",
            "true",
        ),
        (
            "String(Temporal.PlainYearMonth.from('2024-03-01[u-ca=gregory]').equals('2024-03-01[u-ca=hebrew]'))",
            "false",
        ),
    ]);
    expect_throws(
        "RangeError",
        &["Temporal.PlainYearMonth.from({calendar:'nonsense',year:2024,month:3})"],
    );
}

#[test]
fn month_day_non_iso() {
    expect_strings(&[
        (
            "Temporal.PlainMonthDay.from({calendar:'gregory',monthCode:'M02',day:29}).toString()",
            "1972-02-29[u-ca=gregory]",
        ),
        (
            "Temporal.PlainMonthDay.from({monthCode:'M02',day:29}).toString()",
            "02-29",
        ),
        (
            "Temporal.PlainMonthDay.from('1972-02-29[u-ca=gregory]').calendarId",
            "gregory",
        ),
        (
            "Temporal.PlainMonthDay.from({calendar:'hebrew',monthCode:'M05L',day:15}).monthCode",
            "M05L",
        ),
        (
            "Temporal.PlainMonthDay.from({calendar:'gregory',monthCode:'M03',day:15}).toPlainDate({year:2024}).toString()",
            "2024-03-15[u-ca=gregory]",
        ),
        (
            "Temporal.PlainMonthDay.from({calendar:'gregory',monthCode:'M03',day:15}).with({day:20}).toString()",
            "1972-03-20[u-ca=gregory]",
        ),
        (
            "String(Temporal.PlainMonthDay.from('1972-03-15[u-ca=gregory]').equals('1972-03-15[u-ca=gregory]'))",
            "true",
        ),
    ]);
    expect_throws(
        "RangeError",
        &["Temporal.PlainMonthDay.from({calendar:'nonsense',monthCode:'M03',day:15})"],
    );
}

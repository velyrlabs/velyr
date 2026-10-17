//! Temporal.PlainMonthDay (N27): ISO calendar only.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal_month_day.js".into(),
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
fn parses_and_prints_month_days() {
    expect_strings(&[
        ("Temporal.PlainMonthDay.from('--02-29').toString()", "02-29"),
        ("Temporal.PlainMonthDay.from('02-29').toString()", "02-29"),
        (
            "Temporal.PlainMonthDay.from('2024-03-15').toString()",
            "03-15",
        ),
        (
            "Temporal.PlainMonthDay.from({month:2,day:29}).monthCode",
            "M02",
        ),
        (
            "String(Temporal.PlainMonthDay.from({monthCode:'M07',day:4}).day)",
            "4",
        ),
        (
            "Temporal.PlainMonthDay.from({month:2,day:30},{overflow:'constrain'}).toString()",
            "02-29",
        ),
        (
            "Temporal.PlainMonthDay.from({year:2023,month:2,day:29}).toString()",
            "02-28",
        ),
        (
            "Temporal.PlainMonthDay.from(Temporal.PlainMonthDay.from('05-06')).toString()",
            "05-06",
        ),
        (
            "new Temporal.PlainMonthDay(12, 25).toString({calendarName:'always'})",
            "1972-12-25[u-ca=iso8601]",
        ),
        ("new Temporal.PlainMonthDay(2, 29).calendarId", "iso8601"),
        (
            "JSON.stringify(Temporal.PlainMonthDay.from('--12-25'))",
            "\"12-25\"",
        ),
        (
            "Object.prototype.toString.call(new Temporal.PlainMonthDay(1, 1))",
            "[object Temporal.PlainMonthDay]",
        ),
    ]);
}

#[test]
fn converts_to_plain_dates() {
    expect_strings(&[
        (
            "Temporal.PlainMonthDay.from('--02-29').toPlainDate({year:2024}).toString()",
            "2024-02-29",
        ),
        (
            "Temporal.PlainMonthDay.from('--02-29').toPlainDate({year:2023}).toString()",
            "2023-02-28",
        ),
    ]);
}

#[test]
fn with_and_equals() {
    expect_strings(&[
        (
            "Temporal.PlainMonthDay.from('--03-15').with({day:20}).toString()",
            "03-20",
        ),
        (
            "Temporal.PlainMonthDay.from('--03-15').with({monthCode:'M04'}).toString()",
            "04-15",
        ),
        (
            "String(Temporal.PlainMonthDay.from('--03-15').equals('03-15'))",
            "true",
        ),
        (
            "String(Temporal.PlainMonthDay.from('--03-15').equals({month:3,day:16}))",
            "false",
        ),
    ]);
}

#[test]
fn reports_shape_and_errors() {
    expect_strings(&[
        ("String(Temporal.PlainMonthDay.length)", "2"),
        ("String(Temporal.PlainMonthDay.from.length)", "1"),
        ("String(Temporal.PlainMonthDay.prototype.with.length)", "1"),
        (
            "String(Temporal.PlainMonthDay.prototype.equals.length)",
            "1",
        ),
        (
            "String(Temporal.PlainMonthDay.prototype.toPlainDate.length)",
            "1",
        ),
        ("String(typeof Temporal.PlainMonthDay.compare)", "undefined"),
        (
            "String(typeof Temporal.PlainMonthDay.prototype.add)",
            "undefined",
        ),
    ]);
    expect_throws(
        "RangeError",
        &[
            "Temporal.PlainMonthDay.from({monthCode:'M13',day:1})",
            "Temporal.PlainMonthDay.from({month:2,day:30},{overflow:'reject'})",
            "Temporal.PlainMonthDay.from('--13-01')",
            "new Temporal.PlainMonthDay(2, 30)",
            "new Temporal.PlainMonthDay(1, 1, 'nonsense')",
        ],
    );
    expect_throws(
        "TypeError",
        &[
            "Temporal.PlainMonthDay.from('--02-29').valueOf()",
            "Temporal.PlainMonthDay.from({month:2})",
            "Temporal.PlainMonthDay.from(1)",
            "Temporal.PlainMonthDay.from('--02-29').toPlainDate({})",
            "Temporal.PlainMonthDay.from('--02-29').with({})",
            "Temporal.PlainMonthDay()",
            "Temporal.PlainMonthDay.prototype.toString.call({})",
        ],
    );
}

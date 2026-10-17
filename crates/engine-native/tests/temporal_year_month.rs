//! Temporal.PlainYearMonth (N26): ISO calendar only.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal_year_month.js".into(),
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
fn parses_and_prints_year_months() {
    expect_strings(&[
        (
            "Temporal.PlainYearMonth.from('2024-03').toString()",
            "2024-03",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03-15').toString()",
            "2024-03",
        ),
        (
            "Temporal.PlainYearMonth.from({year:2024,month:3}).toString()",
            "2024-03",
        ),
        (
            "Temporal.PlainYearMonth.from({year:2024,monthCode:'M03'}).toString()",
            "2024-03",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03').toString({calendarName:'always'})",
            "2024-03-01[u-ca=iso8601]",
        ),
        ("new Temporal.PlainYearMonth(2024, 3).toJSON()", "2024-03"),
    ]);
}

#[test]
fn arithmetic_and_comparison() {
    expect_strings(&[
        (
            "Temporal.PlainYearMonth.from({year:2024,month:3}).add({months:11}).toString()",
            "2025-02",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03').subtract({years:1}).toString()",
            "2023-03",
        ),
        (
            "String(Temporal.PlainYearMonth.compare(Temporal.PlainYearMonth.from('2024-03'), Temporal.PlainYearMonth.from('2024-04')))",
            "-1",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03').until(Temporal.PlainYearMonth.from('2025-05')).toString()",
            "P1Y2M",
        ),
        (
            "Temporal.PlainYearMonth.from('2025-05').since(Temporal.PlainYearMonth.from('2024-03')).toString()",
            "P1Y2M",
        ),
        (
            "String(Temporal.PlainYearMonth.from('2024-03').equals('2024-03'))",
            "true",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03').with({month:7}).toString()",
            "2024-07",
        ),
        (
            "Temporal.PlainYearMonth.from('2024-03').with({year:2020}).toString()",
            "2020-03",
        ),
    ]);
}

#[test]
fn accessors() {
    expect_strings(&[(
        "const y = Temporal.PlainYearMonth.from('2024-02'); [y.calendarId, y.year, y.month, y.monthCode, y.daysInYear, y.daysInMonth, y.monthsInYear, y.inLeapYear, y.era, y.eraYear].join()",
        "iso8601,2024,2,M02,366,29,12,true,,",
    )]);
}

#[test]
fn converts_to_plain_date() {
    expect_strings(&[
        (
            "new Temporal.PlainYearMonth(2024,3).toPlainDate({day:31}).toString()",
            "2024-03-31",
        ),
        (
            "new Temporal.PlainYearMonth(2023,2).toPlainDate({day:31}).toString()",
            "2023-02-28",
        ),
    ]);
    expect_throws(
        "TypeError",
        &["new Temporal.PlainYearMonth(2024,3).toPlainDate({})"],
    );
}

#[test]
fn rejects_misuse() {
    expect_throws(
        "TypeError",
        &[
            "Temporal.PlainYearMonth.from('2024-03').valueOf()",
            "Temporal.PlainYearMonth(2024, 3)",
            "Temporal.PlainYearMonth.from({year:2024})",
            "Temporal.PlainYearMonth.from('2024-03').with({})",
        ],
    );
    expect_throws(
        "RangeError",
        &[
            "Temporal.PlainYearMonth.from({year:2024,month:13},{overflow:'reject'})",
            "new Temporal.PlainYearMonth(2024, 13)",
            "Temporal.PlainYearMonth.from('+275760-10')",
        ],
    );
    expect_strings(&[
        ("String(Temporal.PlainYearMonth.length)", "2"),
        (
            "Object.prototype.toString.call(new Temporal.PlainYearMonth(2024,3))",
            "[object Temporal.PlainYearMonth]",
        ),
    ]);
}

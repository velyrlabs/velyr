//! Temporal plain types (N23): spec order of operations, calendar and string edge cases.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal_plain_fidelity.js".into(),
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

const LOGGED_BAG: &str = "(() => { const log = []; const bag = new Proxy({ year: 2020, month: 1, day: 1 }, \
    { get(t, k) { log.push(String(k)); return t[k]; } }); \
    const options = new Proxy({}, { get(t, k) { log.push('options.' + String(k)); return t[k]; } }); \
    Temporal.PlainDate.from(bag, options); return log.join(); })()";

#[test]
fn bag_properties_are_read_in_alphabetical_order_before_overflow() {
    expect_strings(&[(
        LOGGED_BAG,
        "calendar,day,month,monthCode,year,options.overflow",
    )]);
}

#[test]
fn time_strings_with_a_utc_designator_are_rejected() {
    expect_throws("RangeError", &["Temporal.PlainTime.from('12:00:00Z')"]);
}

#[test]
fn unicode_minus_sign_is_rejected_in_dates_and_times() {
    expect_throws(
        "RangeError",
        &[
            "Temporal.PlainDate.from('\u{2212}000001-01-01')",
            "Temporal.PlainDateTime.from('1976-11-18T15:23:30.12\u{2212}02:00')",
        ],
    );
}

#[test]
fn round_increment_must_divide_the_unit_evenly() {
    expect_throws(
        "RangeError",
        &[
            "Temporal.PlainDateTime.from({year:2020,month:1,day:1}).round({smallestUnit:'hour',roundingIncrement:5})",
        ],
    );
}

#[test]
fn round_does_not_read_largest_unit() {
    expect_strings(&[(
        "(() => { const log = []; const o = new Proxy({ smallestUnit: 'hour' }, \
         { get(t, k) { log.push(String(k)); return t[k]; } }); \
         Temporal.PlainTime.from('12:30').round(o); return log.join(); })()",
        "roundingIncrement,roundingMode,smallestUnit",
    )]);
}

#[test]
fn bag_calendar_is_validated_and_year_zero_rejected() {
    expect_throws(
        "RangeError",
        &[
            "Temporal.PlainDate.from({year:2020,month:1,day:1,calendar:''})",
            "Temporal.PlainDate.from({year:2020,month:1,day:1,calendar:'\u{130}SO8601'})",
            "Temporal.PlainDate.from('-000000-01-01')",
        ],
    );
    expect_throws(
        "TypeError",
        &["Temporal.PlainDate.from({year:2020,month:1,day:1,calendar:null})"],
    );
}

#[test]
fn with_reads_keys_in_order_and_rejects_calendar_and_time_zone() {
    expect_strings(&[(
        "(() => { const log = []; const bag = new Proxy({ day: 2 }, \
         { get(t, k) { log.push(String(k)); return t[k]; } }); \
         Temporal.PlainDate.from('2020-01-01').with(bag); return log.join(); })()",
        "calendar,timeZone,day,month,monthCode,year",
    )]);
}

#[test]
fn constructors_accept_only_calendar_ids_and_spec_lengths() {
    expect_strings(&[
        (
            "[Temporal.PlainDate.length, Temporal.PlainTime.length, Temporal.PlainDateTime.length].join()",
            "3,0,3",
        ),
        (
            "new Temporal.PlainDate(2000, 5, 2, 'ISO8601').toString()",
            "2000-05-02",
        ),
        (
            "Temporal.PlainDate.from('2020-01-01').withCalendar('2020-01-01T00:00').toString()",
            "2020-01-01",
        ),
    ]);
    expect_throws(
        "RangeError",
        &["new Temporal.PlainDate(2000, 5, 2, '1997-12-04[u-ca=iso8601]')"],
    );
}

#[test]
fn plain_date_time_gains_with_plain_time_and_days_in_week() {
    expect_strings(&[
        (
            "Temporal.PlainDateTime.from('2020-01-01T10:00').withPlainTime('12:30').toString()",
            "2020-01-01T12:30:00",
        ),
        (
            "String(Temporal.PlainDate.from('2020-01-01').daysInWeek)",
            "7",
        ),
    ]);
}

#[test]
fn month_code_syntax_is_checked_when_read_and_objects_need_string_results() {
    expect_throws(
        "TypeError",
        &["Temporal.PlainDate.from({year:2026,monthCode:{toString:()=>5},day:1})"],
    );
    expect_throws(
        "RangeError",
        &["Temporal.PlainDate.from({day:1,monthCode:'L99M',year:Symbol()})"],
    );
}

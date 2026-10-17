//! Temporal bridges (N28): toPlainYearMonth / toPlainMonthDay and the leftover fixes.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal_bridges.js".into(),
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
fn plain_date_bridges() {
    expect_strings(&[
        (
            "Temporal.PlainDate.from('2024-03-15').toPlainYearMonth().toString()",
            "2024-03",
        ),
        (
            "Temporal.PlainDate.from('2024-03-15').toPlainMonthDay().toString()",
            "03-15",
        ),
        (
            "Temporal.PlainDate.from('2024-02-29').toPlainMonthDay().toString()",
            "02-29",
        ),
        (
            "Temporal.PlainDate.from('2023-02-28').toPlainMonthDay().toPlainDate({year:2024}).toString()",
            "2024-02-28",
        ),
        (
            "String(Temporal.PlainDate.prototype.toPlainYearMonth.length)",
            "0",
        ),
        (
            "String(Temporal.PlainDate.prototype.toPlainMonthDay.length)",
            "0",
        ),
    ]);
}

#[test]
fn plain_date_time_and_zoned_bridges() {
    expect_strings(&[
        (
            "Temporal.PlainDateTime.from('2024-03-15T10:00:00').toPlainYearMonth().toString()",
            "2024-03",
        ),
        (
            "Temporal.PlainDateTime.from('2024-03-15T10:00:00').toPlainMonthDay().toString()",
            "03-15",
        ),
        (
            "String(Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC]').toPlainYearMonth().month)",
            "3",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T23:30:00+00:00[UTC]').toPlainMonthDay().toString()",
            "03-15",
        ),
        (
            "Temporal.ZonedDateTime.from('2024-03-15T23:30:00-05:00[America/Bogota]').toPlainMonthDay().toString()",
            "03-15",
        ),
        (
            "String(Temporal.ZonedDateTime.prototype.toPlainYearMonth.length)",
            "0",
        ),
        (
            "String(Temporal.PlainDateTime.prototype.toPlainMonthDay.length)",
            "0",
        ),
    ]);
}

#[test]
fn from_accepts_date_like_objects() {
    expect_strings(&[
        (
            "Temporal.PlainYearMonth.from(Temporal.PlainDate.from('2024-03-15')).toString()",
            "2024-03",
        ),
        (
            "Temporal.PlainYearMonth.from(Temporal.PlainDateTime.from('2024-03-15T10:00')).toString()",
            "2024-03",
        ),
        (
            "Temporal.PlainYearMonth.from(Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC]')).toString()",
            "2024-03",
        ),
        (
            "Temporal.PlainMonthDay.from(Temporal.PlainDate.from('2024-03-15')).toString()",
            "03-15",
        ),
        (
            "Temporal.PlainMonthDay.from(Temporal.PlainDateTime.from('2024-03-15T10:00')).toString()",
            "03-15",
        ),
        (
            "Temporal.PlainMonthDay.from(Temporal.ZonedDateTime.from('2024-03-15T10:00:00+00:00[UTC]')).toString()",
            "03-15",
        ),
    ]);
}

#[test]
fn bridges_brand_check() {
    expect_throws(
        "TypeError",
        &[
            "Temporal.PlainDate.prototype.toPlainYearMonth.call({})",
            "Temporal.PlainDate.prototype.toPlainMonthDay.call({})",
            "Temporal.PlainDateTime.prototype.toPlainYearMonth.call({})",
            "Temporal.ZonedDateTime.prototype.toPlainMonthDay.call({})",
        ],
    );
}

#[test]
fn zoned_duration_argument_uses_exact_fields() {
    expect_strings(&[
        // argument-duration-max: 2**53-ish sub-second fields stay exact
        (
            "Temporal.ZonedDateTime.from('1970-01-01T00:00:00+00:00[UTC]').add({nanoseconds: 9007199254740991}).epochNanoseconds.toString()",
            "9007199254740991",
        ),
        (
            "Temporal.ZonedDateTime.from('1970-01-01T00:00:00+00:00[UTC]').add(new Temporal.Duration(0,0,0,0,0,0,0,0,0,9007199254740991)).epochNanoseconds.toString()",
            "9007199254740991",
        ),
        (
            "Temporal.ZonedDateTime.from('1970-01-01T00:00:00+00:00[UTC]').subtract(new Temporal.Duration(0,0,0,0,0,0,0,0,9007199254740991)).epochNanoseconds.toString()",
            "-9007199254740991000",
        ),
    ]);
}

#[test]
fn common_serializers_brand_check() {
    expect_throws(
        "TypeError",
        &[
            "Temporal.PlainYearMonth.prototype.toJSON.call({})",
            "Temporal.PlainYearMonth.prototype.toLocaleString.call({})",
            "Temporal.PlainMonthDay.prototype.toJSON.call({})",
            "Temporal.PlainMonthDay.prototype.toLocaleString.call(undefined)",
        ],
    );
}

#[test]
fn rounding_increment_coercion_order() {
    // roundingIncrement must be read (and fail) before an invalid smallestUnit is looked at
    // is not observable; the wrong-type case is: a Symbol increment throws TypeError, not RangeError.
    expect_throws(
        "TypeError",
        &[
            "Temporal.PlainYearMonth.from('2024-03').until('2024-05', {roundingIncrement: Symbol()})",
            "Temporal.PlainMonthDay.from('03-15').equals('03-15') ; Temporal.PlainDate.from('2024-03-15').since('2024-03-16', {roundingIncrement: 1n})",
        ],
    );
    expect_strings(&[
        (
            "Temporal.PlainYearMonth.from('2000-05').until('2001-06', {roundingIncrement: true}).toString()",
            "P1Y1M",
        ),
        (
            "Temporal.PlainYearMonth.from('2000-05').until('2001-06', {roundingIncrement: {valueOf() { return 1; }, toString() { return 'x'; }}}).toString()",
            "P1Y1M",
        ),
    ]);
    expect_throws(
        "RangeError",
        &[
            "Temporal.PlainYearMonth.from('2000-05').until('2001-06', {roundingIncrement: {}})",
            "Temporal.PlainYearMonth.from('2000-05').until('2001-06', {roundingIncrement: null})",
            "Temporal.PlainYearMonth.from('2024-03').until('2024-05', {roundingIncrement: 0})",
            "Temporal.PlainYearMonth.from('2024-03').since('2024-05', {roundingIncrement: Infinity})",
        ],
    );
}

#[test]
fn temporal_object_as_calendar_property() {
    expect_strings(&[(
        "Temporal.PlainDate.from('2000-05-02').until({year: 2005, month: 6, day: 2, calendar: Temporal.PlainYearMonth.from('2000-05')}).toString()",
        "P1857D",
    )]);
}

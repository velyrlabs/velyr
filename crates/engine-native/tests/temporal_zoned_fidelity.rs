//! Temporal Instant, ZonedDateTime and Now (N24): transitions, offsets, option order and ranges.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal_zoned_fidelity.js".into(),
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

const LA: &str = "Temporal.ZonedDateTime.from('2024-03-10T12:00:00-07:00[America/Los_Angeles]')";

#[test]
fn time_zone_transitions_are_found_in_both_directions() {
    expect_strings(&[
        (
            &format!("{LA}.getTimeZoneTransition('previous').toString()"),
            "2024-03-10T03:00:00-07:00[America/Los_Angeles]",
        ),
        (
            &format!("{LA}.getTimeZoneTransition('next').toString().slice(0, 10)"),
            "2024-11-03",
        ),
        (
            "String(Temporal.ZonedDateTime.from('2024-01-01T00:00:00+00:00[UTC]').getTimeZoneTransition('next'))",
            "null",
        ),
        (
            "String(Temporal.ZonedDateTime.from('2024-01-01T00:00:00+01:00[+01:00]').getTimeZoneTransition('previous'))",
            "null",
        ),
    ]);
    expect_throws(
        "RangeError",
        &[&format!("{LA}.getTimeZoneTransition('later')")],
    );
    expect_strings(&[(
        &format!("{LA}.getTimeZoneTransition({{ direction: 'previous' }}).hour + ''"),
        "3",
    )]);
    expect_throws(
        "TypeError",
        &[
            &format!("{LA}.getTimeZoneTransition(5)"),
            &format!("{LA}.getTimeZoneTransition(undefined)"),
        ],
    );
}

#[test]
fn with_plain_time_and_with_calendar_exist() {
    expect_strings(&[
        (
            &format!("{LA}.withPlainTime('02:30').toString()"),
            "2024-03-10T03:30:00-07:00[America/Los_Angeles]",
        ),
        (
            &format!("{LA}.withPlainTime().toString()"),
            "2024-03-10T00:00:00-08:00[America/Los_Angeles]",
        ),
        (
            &format!("{LA}.withCalendar('ISO8601').calendarId"),
            "iso8601",
        ),
    ]);
}

#[test]
fn instant_parsing_and_ranges() {
    expect_throws(
        "RangeError",
        &[
            "Temporal.Instant.from('2024-01-01T00:00:00')",
            "Temporal.Instant.fromEpochMilliseconds(8.64e15 + 1)",
            "Temporal.Instant.fromEpochMilliseconds(1.3)",
            "Temporal.Instant.from({ toString() { return 'not a date'; } })",
        ],
    );
    expect_throws("TypeError", &["Temporal.Instant.fromEpochMilliseconds(1n)"]);
    expect_strings(&[
        (
            "Temporal.Instant.fromEpochMilliseconds(8.64e15).epochMilliseconds + ''",
            "8640000000000000",
        ),
        ("new Temporal.Instant(true).epochNanoseconds + ''", "1"),
    ]);
}

#[test]
fn now_functions_have_spec_names_and_lengths() {
    expect_strings(&[
        (
            "[Temporal.Now.instant.name, Temporal.Now.zonedDateTimeISO.name, Temporal.Now.zonedDateTimeISO.length].join()",
            "instant,zonedDateTimeISO,0",
        ),
        (
            "typeof Temporal.Now.getTimeZoneTransition + Object.keys(Temporal.Now).length",
            "undefined0",
        ),
    ]);
}

#[test]
fn round_reads_options_in_spec_order_without_prototype_pollution() {
    expect_strings(&[(
        "(() => { const log = []; const o = new Proxy({ smallestUnit: 'hour' }, \
         { get(t, k) { log.push(String(k)); return t[k]; } }); \
         Temporal.Instant.from('2020-01-01T00:00Z').round(o); return log.join(); })()",
        "roundingIncrement,roundingMode,smallestUnit",
    )]);
    expect_throws(
        "RangeError",
        &[
            "Temporal.Instant.from('2020-01-01T00:00Z').round({ smallestUnit: 'hour', roundingIncrement: null })",
        ],
    );
}

#[test]
fn zoned_from_validates_offset_disambiguation_and_calendar() {
    expect_throws(
        "RangeError",
        &[
            "Temporal.ZonedDateTime.from('2020-01-01T00:00[UTC]', { disambiguation: 'sometimes' })",
            "Temporal.ZonedDateTime.from({ year: 2020, month: 1, day: 1, timeZone: 'UTC', calendar: '' })",
            "Temporal.ZonedDateTime.from({ year: 2020, month: 1, day: 1, timeZone: 'UTC', offset: '0:00' })",
        ],
    );
    expect_throws(
        "TypeError",
        &[
            "Temporal.ZonedDateTime.from({ year: 2020, month: 1, day: 1, timeZone: 'UTC', offset: null })",
        ],
    );
    expect_throws(
        "TypeError",
        &[
            "Temporal.ZonedDateTime.from({ year: 2020, month: 1, day: 1, timeZone: 'UTC', calendar: null })",
        ],
    );
    expect_strings(&[(
        "Temporal.ZonedDateTime.from({ year: 2020, month: 1, day: 1, timeZone: '2021-08-19T17:30:45.123456789[America/Vancouver]' }).timeZoneId",
        "America/Vancouver",
    )]);
}

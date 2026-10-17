//! Temporal `toLocaleString` and Temporal inputs to `Intl.DateTimeFormat`.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn expect_true(cases: &[&str]) {
    let mut iso = isolate();
    for code in cases {
        let source = Source {
            name: "temporal_tolocalestring.js".into(),
            text: (*code).into(),
        };
        match iso.eval_script(&source) {
            Ok(value) => assert_eq!(value, Value::Bool(true), "{code}"),
            Err(error) => panic!("`{code}` threw {}", error.message),
        }
    }
}

#[test]
fn plain_types_format_their_own_fields() {
    expect_true(&[
        "Temporal.PlainDate.from('2024-03-15').toLocaleString('en') === '3/15/2024'",
        "Temporal.PlainDate.from('2024-03-15').toLocaleString('en',{timeZone:'Asia/Tokyo'}) === '3/15/2024'",
        "Temporal.PlainTime.from('13:05:09').toLocaleString('en') === '1:05:09\u{202f}PM'",
        "Temporal.PlainDateTime.from('2024-03-15T13:05:09').toLocaleString('en') === '3/15/2024, 1:05:09\u{202f}PM'",
        "new Temporal.PlainYearMonth(2024, 3, 'gregory').toLocaleString('en') === '3/2024'",
        "new Temporal.PlainMonthDay(3, 15, 'gregory').toLocaleString('en') === '3/15'",
        "Temporal.PlainDate.from('2024-03-15').toLocaleString('en',{dateStyle:'long'}) === 'March 15, 2024'",
        "new Intl.DateTimeFormat('en').format(Temporal.PlainTime.from('13:05:09')) === '1:05:09\u{202f}PM'",
        "Temporal.PlainDateTime.from('2024-03-15T00:05:09').toLocaleString('en',{hourCycle:'h24',timeZone:'UTC'}).includes('24:05:09')",
        "new Temporal.ZonedDateTime(0n, '+00:00').toLocaleString('en').endsWith('GMT')",
    ]);
}

#[test]
fn instant_and_zoned_use_a_time_zone() {
    expect_true(&[
        "Temporal.Instant.from('2024-03-15T13:05:09Z').toLocaleString('en',{timeZone:'UTC'}) === '3/15/2024, 1:05:09\u{202f}PM'",
        "Temporal.ZonedDateTime.from('2024-03-15T13:05:09+00:00[UTC]').toLocaleString('en') === '3/15/2024, 1:05:09\u{202f}PM UTC'",
        "Temporal.ZonedDateTime.from('2024-03-15T13:05:09+09:00[Asia/Tokyo]').toLocaleString('en',{timeZoneName:'long'}).endsWith('Japan Standard Time')",
        "(() => { try { Temporal.ZonedDateTime.from('2024-03-15T13:05:09+00:00[UTC]').toLocaleString('en',{timeZone:'Asia/Tokyo'}); } catch (e) { return e instanceof TypeError; } })()",
    ]);
}

#[test]
fn date_time_format_accepts_temporal_inputs() {
    expect_true(&[
        "new Intl.DateTimeFormat('en').format(Temporal.PlainDate.from('2024-03-15')) === '3/15/2024'",
        "new Intl.DateTimeFormat('en').formatToParts(Temporal.PlainDate.from('2024-03-15')).map(p => p.type).join() === 'month,literal,day,literal,year'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC'}).format(Temporal.Instant.from('2024-03-15T13:05:09Z')) === '3/15/2024, 1:05:09\u{202f}PM'",
        "new Intl.DateTimeFormat('en').formatRange(Temporal.PlainDate.from('2024-03-15'), Temporal.PlainDate.from('2024-03-17')).includes('3/17/2024')",
        "new Intl.DateTimeFormat('en',{year:'numeric'}).format(new Temporal.PlainDate(275760, 9, 13)) === '275760'",
    ]);
}

#[test]
fn mismatches_throw() {
    let throws = |code: &str, kind: &str| {
        format!(
            "(() => {{ try {{ {code}; }} catch (e) {{ return e instanceof {kind}; }} return false; }})()"
        )
    };
    let cases = [
        throws(
            "Temporal.PlainYearMonth.from('2024-03').toLocaleString('en')",
            "RangeError",
        ),
        throws(
            "Temporal.PlainDate.from('2024-03-15').withCalendar('japanese').toLocaleString('en')",
            "RangeError",
        ),
        throws(
            "new Intl.DateTimeFormat('en').format(Temporal.ZonedDateTime.from('2024-03-15T13:05:09+00:00[UTC]'))",
            "TypeError",
        ),
        throws(
            "new Intl.DateTimeFormat('en',{year:'numeric'}).format(Temporal.PlainTime.from('13:05:09'))",
            "TypeError",
        ),
        throws(
            "Temporal.PlainDate.from('2024-03-15').toLocaleString('en',{timeStyle:'short'})",
            "TypeError",
        ),
        throws(
            "new Intl.DateTimeFormat('en').formatRange(Temporal.PlainDate.from('2024-03-15'), Temporal.PlainTime.from('13:05:09'))",
            "TypeError",
        ),
        throws(
            "new Intl.DateTimeFormat('en').formatRange(Temporal.PlainDate.from('2024-03-15'), 0)",
            "TypeError",
        ),
    ];
    let refs: Vec<&str> = cases.iter().map(String::as_str).collect();
    expect_true(&refs);
}

#[test]
fn duration_to_locale_string_uses_duration_format() {
    expect_true(&[
        "Temporal.Duration.from({hours:1,minutes:2,seconds:3}).toLocaleString('en') === '1 hr, 2 min, 3 sec'",
        "Temporal.Duration.from({hours:1}).toLocaleString('en',{style:'long'}) === '1 hour'",
    ]);
}

fn run(iso: &mut NativeIsolate, text: &str) {
    let source = Source {
        name: "lazy.js".into(),
        text: text.into(),
    };
    iso.eval_script(&source)
        .unwrap_or_else(|e| panic!("`{text}` threw {}", e.message));
}

#[test]
fn intl_stays_lazy_until_to_locale_string() {
    let mut iso = isolate();
    run(
        &mut iso,
        "globalThis.d = Temporal.PlainDate.from('2024-03-15'); typeof Intl",
    );
    assert_eq!(iso.pending_lazy_globals(), ["Intl"]);
    run(&mut iso, "d.toLocaleString('en')");
    assert!(iso.pending_lazy_globals().is_empty());
}

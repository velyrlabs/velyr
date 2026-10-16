//! Temporal on engine-native (VELYR-173), built on `temporal_rs`: expectations follow the
//! spec / V8. Wall-clock results (`Temporal.Now`) are checked for sanity only.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "temporal.js".into(),
        text: code.into(),
    };
    match iso.eval_script(&source) {
        Ok(value) => value,
        Err(error) => panic!("`{code}` threw {}", error.message),
    }
}

fn text(s: &str) -> Value {
    Value::String(JsString::from(s))
}

/// Evaluates each `(code, expected string)` pair in one isolate.
fn expect_strings(cases: &[(&str, &str)]) {
    let mut iso = isolate();
    for (code, expected) in cases {
        assert_eq!(eval(&mut iso, code), text(expected), "{code}");
    }
}

/// Like `expect_strings`, after running `setup` once in the isolate.
fn expect_strings_after(setup: &str, cases: &[(&str, &str)]) {
    let mut iso = isolate();
    eval(&mut iso, setup);
    for (code, expected) in cases {
        assert_eq!(eval(&mut iso, code), text(expected), "{code}");
    }
}

fn expect_true_after(setup: &str, cases: &[&str]) {
    let mut iso = isolate();
    eval(&mut iso, setup);
    for code in cases {
        let block = format!("{{ {code} }}");
        assert_eq!(eval(&mut iso, &block), Value::Bool(true), "{code}");
    }
}

fn expect_true(cases: &[&str]) {
    let mut iso = isolate();
    for code in cases {
        let block = format!("{{ {code} }}");
        assert_eq!(eval(&mut iso, &block), Value::Bool(true), "{code}");
    }
}

/// `code` must throw an instance of the error constructor `kind`.
fn expect_throws(kind: &str, cases: &[&str]) {
    let mut iso = isolate();
    for code in cases {
        let probe = format!(
            "(() => {{ try {{ {code}; return 'no throw'; }} catch (e) {{ return e instanceof {kind} ? 'ok' : String(e); }} }})()"
        );
        assert_eq!(eval(&mut iso, &probe), text("ok"), "{code}");
    }
}

#[test]
fn namespace_exposes_all_classes() {
    expect_true(&[
        "typeof Temporal === 'object'",
        "Object.prototype.toString.call(Temporal) === '[object Temporal]'",
        "['PlainDate','PlainTime','PlainDateTime','ZonedDateTime','Instant','Duration'].every(n => typeof Temporal[n] === 'function')",
        "typeof Temporal.Now === 'object'",
        "Object.prototype.toString.call(new Temporal.PlainDate(2020, 1, 1)) === '[object Temporal.PlainDate]'",
    ]);
}

#[test]
fn plain_date_construction_and_from() {
    expect_strings(&[
        (
            "new Temporal.PlainDate(2020, 2, 29).toString()",
            "2020-02-29",
        ),
        (
            "Temporal.PlainDate.from('2020-02-29T10:30:00').toString()",
            "2020-02-29",
        ),
        (
            "Temporal.PlainDate.from({ year: 2021, month: 2, day: 30 }).toString()",
            "2021-02-28",
        ),
        (
            "Temporal.PlainDate.from(new Temporal.PlainDate(2000, 5, 6)).toString()",
            "2000-05-06",
        ),
        ("new Temporal.PlainDate(2020, 2, 29).monthCode", "M02"),
        (
            "new Temporal.PlainDate(-1, 1, 1).toString()",
            "-000001-01-01",
        ),
    ]);
    expect_true(&[
        "const d = new Temporal.PlainDate(2020, 2, 29); d.year === 2020 && d.month === 2 && d.day === 29",
        "const d = new Temporal.PlainDate(2020, 2, 29); d.dayOfWeek === 6 && d.dayOfYear === 60 && d.daysInMonth === 29",
        "new Temporal.PlainDate(2020, 2, 29).inLeapYear && !new Temporal.PlainDate(2021, 2, 1).inLeapYear",
        "new Temporal.PlainDate(2020, 2, 29).calendarId === 'iso8601'",
    ]);
    expect_throws(
        "RangeError",
        &[
            "new Temporal.PlainDate(2020, 13, 1)",
            "Temporal.PlainDate.from('nope')",
            "Temporal.PlainDate.from({ year: 2021, month: 2, day: 30 }, { overflow: 'reject' })",
        ],
    );
    expect_throws(
        "TypeError",
        &[
            "Temporal.PlainDate(2020, 1, 1)",
            "Temporal.PlainDate.from({ year: 2020 })",
        ],
    );
}

#[test]
fn plain_date_arithmetic_compare_and_with() {
    expect_strings(&[
        (
            "new Temporal.PlainDate(2020, 2, 29).add({ years: 1 }).toString()",
            "2021-02-28",
        ),
        (
            "new Temporal.PlainDate(2020, 1, 31).add({ months: 1 }).toString()",
            "2020-02-29",
        ),
        (
            "new Temporal.PlainDate(2020, 3, 1).subtract({ days: 60 }).toString()",
            "2020-01-01",
        ),
        (
            "new Temporal.PlainDate(2020, 1, 1).until(new Temporal.PlainDate(2020, 3, 1)).toString()",
            "P60D",
        ),
        (
            "new Temporal.PlainDate(2020, 1, 1).until(new Temporal.PlainDate(2020, 3, 1), { largestUnit: 'month' }).toString()",
            "P2M",
        ),
        (
            "new Temporal.PlainDate(2020, 3, 1).since(new Temporal.PlainDate(2020, 1, 1)).toString()",
            "P60D",
        ),
        (
            "new Temporal.PlainDate(2020, 2, 29).with({ day: 1 }).toString()",
            "2020-02-01",
        ),
        (
            "new Temporal.PlainDate(2020, 2, 29).toPlainDateTime('10:30').toString()",
            "2020-02-29T10:30:00",
        ),
    ]);
    expect_true(&[
        "Temporal.PlainDate.compare(new Temporal.PlainDate(2020, 1, 1), new Temporal.PlainDate(2020, 1, 2)) === -1",
        "Temporal.PlainDate.compare('2020-01-02', '2020-01-02') === 0",
        "new Temporal.PlainDate(2020, 1, 1).equals('2020-01-01')",
        "!new Temporal.PlainDate(2020, 1, 1).equals('2020-01-02')",
    ]);
}

#[test]
fn plain_time() {
    expect_strings(&[
        (
            "new Temporal.PlainTime(13, 30, 15, 250).toString()",
            "13:30:15.25",
        ),
        ("new Temporal.PlainTime().toString()", "00:00:00"),
        ("Temporal.PlainTime.from('08:05').toString()", "08:05:00"),
        (
            "Temporal.PlainTime.from({ hour: 7, minute: 5 }).toString()",
            "07:05:00",
        ),
        (
            "new Temporal.PlainTime(13, 30, 15).add({ hours: 12 }).toString()",
            "01:30:15",
        ),
        (
            "new Temporal.PlainTime(1, 0).subtract({ minutes: 30 }).toString()",
            "00:30:00",
        ),
        (
            "new Temporal.PlainTime(8, 0).until(new Temporal.PlainTime(10, 30)).toString()",
            "PT2H30M",
        ),
        (
            "new Temporal.PlainTime(10, 30).since(new Temporal.PlainTime(8, 0)).toString()",
            "PT2H30M",
        ),
        (
            "new Temporal.PlainTime(13, 40).round({ smallestUnit: 'hour' }).toString()",
            "14:00:00",
        ),
        (
            "new Temporal.PlainTime(13, 40).with({ minute: 5 }).toString()",
            "13:05:00",
        ),
        (
            "new Temporal.PlainTime(13, 40, 1).toString({ smallestUnit: 'minute' })",
            "13:40",
        ),
    ]);
    expect_true(&[
        "const t = new Temporal.PlainTime(1, 2, 3, 4, 5, 6); t.hour === 1 && t.minute === 2 && t.second === 3 && t.millisecond === 4 && t.microsecond === 5 && t.nanosecond === 6",
        "Temporal.PlainTime.compare('10:00', '09:59') === 1",
        "new Temporal.PlainTime(10).equals('10:00')",
    ]);
    expect_throws(
        "RangeError",
        &[
            "new Temporal.PlainTime(24)",
            "Temporal.PlainTime.from('25:00')",
        ],
    );
}

#[test]
fn plain_date_time() {
    expect_strings(&[
        (
            "new Temporal.PlainDateTime(2020, 2, 29, 12, 30, 15, 250).toString()",
            "2020-02-29T12:30:15.25",
        ),
        (
            "new Temporal.PlainDateTime(2020, 2, 29).toString()",
            "2020-02-29T00:00:00",
        ),
        (
            "Temporal.PlainDateTime.from('2020-02-29T12:30').toString()",
            "2020-02-29T12:30:00",
        ),
        (
            "Temporal.PlainDateTime.from({ year: 2020, month: 2, day: 29, hour: 1 }).toString()",
            "2020-02-29T01:00:00",
        ),
        (
            "new Temporal.PlainDateTime(2020, 2, 28, 23).add({ hours: 2 }).toString()",
            "2020-02-29T01:00:00",
        ),
        (
            "new Temporal.PlainDateTime(2020, 3, 1).subtract({ days: 1, hours: 1 }).toString()",
            "2020-02-28T23:00:00",
        ),
        (
            "new Temporal.PlainDateTime(2020, 1, 1).until(new Temporal.PlainDateTime(2020, 1, 2, 6)).toString()",
            "P1DT6H",
        ),
        (
            "new Temporal.PlainDateTime(2020, 1, 2, 6).since(new Temporal.PlainDateTime(2020, 1, 1)).toString()",
            "P1DT6H",
        ),
        (
            "new Temporal.PlainDateTime(2020, 1, 1, 10, 40).round({ smallestUnit: 'hour' }).toString()",
            "2020-01-01T11:00:00",
        ),
        (
            "new Temporal.PlainDateTime(2020, 1, 1, 10, 40).toPlainDate().toString()",
            "2020-01-01",
        ),
        (
            "new Temporal.PlainDateTime(2020, 1, 1, 10, 40).toPlainTime().toString()",
            "10:40:00",
        ),
        (
            "new Temporal.PlainDateTime(2020, 1, 1, 10, 40).with({ day: 5 }).toString()",
            "2020-01-05T10:40:00",
        ),
        (
            "new Temporal.PlainDateTime(2020, 1, 1, 10).toZonedDateTime('Asia/Tokyo').toString()",
            "2020-01-01T10:00:00+09:00[Asia/Tokyo]",
        ),
    ]);
    expect_true(&[
        "Temporal.PlainDateTime.compare('2020-01-01T10:00', '2020-01-01T09:00') === 1",
        "new Temporal.PlainDateTime(2020, 1, 1, 10).equals('2020-01-01T10:00:00')",
        "const d = new Temporal.PlainDateTime(2020, 2, 29, 1, 2, 3); d.year === 2020 && d.hour === 1 && d.second === 3 && d.daysInYear === 366",
    ]);
}

#[test]
fn instant() {
    expect_strings(&[
        (
            "Temporal.Instant.fromEpochMilliseconds(0).toString()",
            "1970-01-01T00:00:00Z",
        ),
        (
            "Temporal.Instant.fromEpochNanoseconds(1n).toString()",
            "1970-01-01T00:00:00.000000001Z",
        ),
        (
            "new Temporal.Instant(1000000000n).toString()",
            "1970-01-01T00:00:01Z",
        ),
        (
            "Temporal.Instant.from('2020-01-01T00:00:00+01:00').toString()",
            "2019-12-31T23:00:00Z",
        ),
        (
            "Temporal.Instant.from('2020-01-01T00:00:00Z').add({ hours: 1 }).toString()",
            "2020-01-01T01:00:00Z",
        ),
        (
            "Temporal.Instant.from('2020-01-01T00:00:00Z').subtract({ minutes: 1 }).toString()",
            "2019-12-31T23:59:00Z",
        ),
        (
            "Temporal.Instant.from('2020-01-01T00:00:00Z').until('2020-01-01T01:00:00Z').toString()",
            "PT3600S",
        ),
        (
            "Temporal.Instant.from('2020-01-01T00:00:00Z').until('2020-01-01T01:00:00Z', { largestUnit: 'hour' }).toString()",
            "PT1H",
        ),
        (
            "Temporal.Instant.from('2020-01-01T01:00:00Z').since('2020-01-01T00:00:00Z', { largestUnit: 'hour' }).toString()",
            "PT1H",
        ),
        (
            "Temporal.Instant.from('2020-01-01T00:40:00Z').round({ smallestUnit: 'hour' }).toString()",
            "2020-01-01T01:00:00Z",
        ),
        (
            "Temporal.Instant.fromEpochMilliseconds(0).toString({ timeZone: 'America/Sao_Paulo' })",
            "1969-12-31T21:00:00-03:00",
        ),
        (
            "Temporal.Instant.fromEpochMilliseconds(0).toZonedDateTimeISO('Asia/Tokyo').toString()",
            "1970-01-01T09:00:00+09:00[Asia/Tokyo]",
        ),
    ]);
    expect_true(&[
        "Temporal.Instant.from('2020-01-01T00:00:00Z').epochMilliseconds === 1577836800000",
        "Temporal.Instant.fromEpochNanoseconds(5n).epochNanoseconds === 5n",
        "Temporal.Instant.compare('2020-01-01T00:00:00Z', '2020-01-01T00:00:01Z') === -1",
        "Temporal.Instant.from('2020-01-01T00:00:00Z').equals('2020-01-01T01:00:00+01:00')",
    ]);
    expect_throws(
        "RangeError",
        &[
            "Temporal.Instant.from('2020-01-01')",
            "Temporal.Instant.fromEpochNanoseconds(10n ** 30n)",
        ],
    );
}

#[test]
fn duration() {
    expect_strings(&[
        ("new Temporal.Duration(1, 2, 0, 3).toString()", "P1Y2M3D"),
        ("new Temporal.Duration().toString()", "PT0S"),
        ("Temporal.Duration.from('PT1H30M').toString()", "PT1H30M"),
        (
            "Temporal.Duration.from({ hours: 1, minutes: 30 }).toString()",
            "PT1H30M",
        ),
        (
            "Temporal.Duration.from('PT1H').add('PT30M').toString()",
            "PT1H30M",
        ),
        (
            "Temporal.Duration.from('PT1H30M').subtract('PT30M').toString()",
            "PT1H",
        ),
        ("Temporal.Duration.from('P1D').negated().toString()", "-P1D"),
        ("Temporal.Duration.from('-P1D').abs().toString()", "P1D"),
        (
            "Temporal.Duration.from('PT1H30M').with({ hours: 5 }).toString()",
            "PT5H30M",
        ),
        (
            "Temporal.Duration.from('PT1H30M').round({ smallestUnit: 'hour' }).toString()",
            "PT2H",
        ),
        (
            "Temporal.Duration.from('PT90M').round({ largestUnit: 'hour', smallestUnit: 'minute' }).toString()",
            "PT1H30M",
        ),
        (
            "Temporal.Duration.from('PT1.5S').toString({ fractionalSecondDigits: 3 })",
            "PT1.500S",
        ),
    ]);
    expect_true(&[
        "const d = Temporal.Duration.from('P1Y2M3W4DT5H6M7S'); d.years === 1 && d.months === 2 && d.weeks === 3 && d.days === 4 && d.hours === 5 && d.minutes === 6 && d.seconds === 7",
        "Temporal.Duration.from('-PT1S').sign === -1 && Temporal.Duration.from('PT0S').blank",
        "Temporal.Duration.from('PT1H30M').total({ unit: 'minute' }) === 90",
        "Temporal.Duration.from('P1D').total({ unit: 'hour', relativeTo: '2020-01-01' }) === 24",
        "Temporal.Duration.compare('PT1H', 'PT59M') === 1",
        "Temporal.Duration.compare('PT1H', 'PT60M') === 0",
    ]);
    expect_throws(
        "RangeError",
        &[
            "new Temporal.Duration(1, -1)",
            "Temporal.Duration.from('P1Y').total({ unit: 'day' })",
            "Temporal.Duration.from('nope')",
        ],
    );
}

#[test]
fn zoned_date_time_with_named_zone() {
    const LA: &str =
        "var la = Temporal.ZonedDateTime.from('2020-03-08T01:30:00-08:00[America/Los_Angeles]')";
    expect_strings_after(
        LA,
        &[
            (
                "new Temporal.ZonedDateTime(0n, 'America/Sao_Paulo').toString()",
                "1969-12-31T21:00:00-03:00[America/Sao_Paulo]",
            ),
            (
                "Temporal.ZonedDateTime.from({ year: 2020, month: 1, day: 1, timeZone: 'Asia/Tokyo' }).toString()",
                "2020-01-01T00:00:00+09:00[Asia/Tokyo]",
            ),
            (
                "Temporal.ZonedDateTime.from('2020-06-01T12:00:00[Europe/Paris]').toString()",
                "2020-06-01T12:00:00+02:00[Europe/Paris]",
            ),
            // The spring-forward gap: +1 hour is exact time, +1 day is wall-clock time.
            (
                "la.add({ hours: 1 }).toString()",
                "2020-03-08T03:30:00-07:00[America/Los_Angeles]",
            ),
            (
                "la.add({ days: 1 }).toString()",
                "2020-03-09T01:30:00-07:00[America/Los_Angeles]",
            ),
            (
                "la.subtract({ hours: 25 }).toString()",
                "2020-03-07T00:30:00-08:00[America/Los_Angeles]",
            ),
            (
                "la.until(Temporal.ZonedDateTime.from('2020-03-08T03:30:00-07:00[America/Los_Angeles]'), { largestUnit: 'hour' }).toString()",
                "PT1H",
            ),
            (
                "Temporal.ZonedDateTime.from('2020-03-08T03:30:00-07:00[America/Los_Angeles]').since(la, { largestUnit: 'hour' }).toString()",
                "PT1H",
            ),
            (
                "la.startOfDay().toString()",
                "2020-03-08T00:00:00-08:00[America/Los_Angeles]",
            ),
            (
                "la.round({ smallestUnit: 'hour' }).toString()",
                "2020-03-08T03:00:00-07:00[America/Los_Angeles]",
            ),
            (
                "la.with({ hour: 5 }).toString()",
                "2020-03-08T05:30:00-07:00[America/Los_Angeles]",
            ),
            (
                "la.withTimeZone('UTC').toString()",
                "2020-03-08T09:30:00+00:00[UTC]",
            ),
            ("la.toInstant().toString()", "2020-03-08T09:30:00Z"),
            ("la.toPlainDate().toString()", "2020-03-08"),
            ("la.toPlainTime().toString()", "01:30:00"),
            ("la.toPlainDateTime().toString()", "2020-03-08T01:30:00"),
            (
                "la.toString({ timeZoneName: 'never', offset: 'never' })",
                "2020-03-08T01:30:00",
            ),
            ("la.timeZoneId", "America/Los_Angeles"),
            ("la.offset", "-08:00"),
        ],
    );
    expect_true_after(
        LA,
        &[
            "la.hoursInDay === 23",
            "la.offsetNanoseconds === -8 * 3600e9",
            "la.epochMilliseconds === 1583659800000",
            "la.epochNanoseconds === 1583659800000000000n",
            "la.year === 2020 && la.month === 3 && la.day === 8 && la.hour === 1 && la.minute === 30",
            "la.equals('2020-03-08T09:30:00+00:00[UTC]') === false",
            "la.equals('2020-03-08T01:30:00-08:00[America/Los_Angeles]')",
            "Temporal.ZonedDateTime.compare(la, '2020-03-08T09:31:00+00:00[UTC]') === -1",
            "la.calendarId === 'iso8601' && la.dayOfWeek === 7",
        ],
    );
    expect_throws(
        "RangeError",
        &[
            "new Temporal.ZonedDateTime(0n, 'Nowhere/Land')",
            "Temporal.ZonedDateTime.from('2020-01-01T00:00:00')",
        ],
    );
}

#[test]
fn now_returns_sane_values() {
    expect_true(&[
        "Math.abs(Temporal.Now.instant().epochMilliseconds - Date.now()) < 5000",
        "typeof Temporal.Now.timeZoneId() === 'string' && Temporal.Now.timeZoneId().length > 0",
        "Temporal.Now.zonedDateTimeISO('UTC').timeZoneId === 'UTC'",
        "Temporal.Now.zonedDateTimeISO().timeZoneId === Temporal.Now.timeZoneId()",
        "/^\\d{4}-\\d{2}-\\d{2}$/.test(Temporal.Now.plainDateISO('UTC').toString())",
        "/^\\d{2}:\\d{2}:\\d{2}/.test(Temporal.Now.plainTimeISO('UTC').toString())",
        "/^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}/.test(Temporal.Now.plainDateTimeISO('UTC').toString())",
        "Math.abs(Temporal.Now.zonedDateTimeISO('Asia/Tokyo').epochMilliseconds - Date.now()) < 5000",
    ]);
}

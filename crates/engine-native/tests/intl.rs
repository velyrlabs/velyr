//! Date and Intl on engine-native (VELYR-114): expectations follow V8's output.
//! Local-time results depend on the host time zone, so they are checked for consistency;
//! exact strings use UTC or an explicit `timeZone`.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "intl.js".into(),
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

fn expect_true(cases: &[&str]) {
    let mut iso = isolate();
    for code in cases {
        assert_eq!(eval(&mut iso, code), Value::Bool(true), "{code}");
    }
}

#[test]
fn date_constructor_and_utc_fields() {
    expect_strings(&[
        (
            "new Date(Date.UTC(2020, 1, 29, 12, 30, 15, 250)).toISOString()",
            "2020-02-29T12:30:15.250Z",
        ),
        (
            "new Date(Date.UTC(-1, 0, 1)).toISOString()",
            "-000001-01-01T00:00:00.000Z",
        ),
        (
            "new Date(Date.UTC(275760, 8, 13)).toISOString()",
            "+275760-09-13T00:00:00.000Z",
        ),
        ("new Date(0).toUTCString()", "Thu, 01 Jan 1970 00:00:00 GMT"),
        (
            "JSON.stringify(new Date(0))",
            "\"1970-01-01T00:00:00.000Z\"",
        ),
        ("String(new Date(NaN))", "Invalid Date"),
        (
            "Object.prototype.toString.call(new Date(0))",
            "[object Date]",
        ),
        ("typeof Date()", "string"),
        (
            "const d = new Date(Date.UTC(2021, 0, 31)); d.setUTCMonth(1); d.toISOString()",
            "2021-03-03T00:00:00.000Z",
        ),
        (
            "const e = new Date(0); e.setUTCFullYear(2000, 5, 15); e.setUTCHours(25); e.toISOString()",
            "2000-06-16T01:00:00.000Z",
        ),
        (
            "[Date.length, Date.UTC.length, Date.prototype.setHours.length].join()",
            "7,7,4",
        ),
    ]);
    expect_true(&[
        "isNaN(new Date(8.64e15 + 1).getTime()) && new Date(8.64e15).getTime() === 8.64e15",
        "new Date(Date.UTC(2020, 1, 29)).getUTCDay() === 6",
        "new Date(new Date(5)).getTime() === 5 && new Date('1970-01-01T00:00:00.007Z') - 0 === 7",
        "typeof (new Date(0) + 1) === 'string' && new Date(3) * 1 === 3",
        "new Date(0)[Symbol.toPrimitive]('number') === 0",
        "(() => { try { new Date(NaN).toISOString(); } catch (e) { return e instanceof RangeError; } })()",
        "(() => { try { Date.prototype.getTime.call({}); } catch (e) { return e instanceof TypeError; } })()",
        "class D extends Date {}; new D(0).getTime() === 0 && new D(0) instanceof Date",
        "Math.abs(Date.now() - new Date().getTime()) < 1000",
    ]);
}

#[test]
fn date_local_time_is_consistent_with_the_offset() {
    expect_true(&[
        "const l = new Date(2020, 0, 1); l.getTime() - Date.UTC(2020, 0, 1) === l.getTimezoneOffset() * 60000",
        "const m = new Date(2020, 6, 1, 13, 45); m.getHours() === 13 && m.getMinutes() === 45 && m.getMonth() === 6",
        "new Date(2020, 0, 1).getFullYear() === 2020 && new Date(2020, 0, 1).getDate() === 1",
        "const n = new Date(2020, 0, 31); n.setMonth(1); n.getMonth() === 2 && n.getDate() === 2",
        "/^[A-Z][a-z]{2} [A-Z][a-z]{2} \\d\\d \\d{4} \\d\\d:\\d\\d:\\d\\d GMT[+-]\\d{4}( \\(.+\\))?$/.test(new Date(0).toString())",
        "/^[A-Z][a-z]{2} [A-Z][a-z]{2} \\d\\d \\d{4}$/.test(new Date(0).toDateString())",
        "/^\\d\\d:\\d\\d:\\d\\d GMT[+-]\\d{4}/.test(new Date(0).toTimeString())",
    ]);
}

#[test]
fn date_parse_accepts_iso_and_v8_formats() {
    expect_true(&[
        "Date.parse('2020-02-29T12:30:15.250Z') === 1582979415250",
        "Date.parse('2020-02-29') === Date.UTC(2020, 1, 29)",
        "Date.parse('2020-02') === Date.UTC(2020, 1) && Date.parse('2020') === Date.UTC(2020, 0)",
        "Date.parse('2020-02-29T00:00') === new Date(2020, 1, 29).getTime()",
        "Date.parse('2020-02-29T12:00:00+02:00') === Date.UTC(2020, 1, 29, 10)",
        "Date.parse('+275760-09-13T00:00:00.000Z') === 8.64e15",
        "isNaN(Date.parse('2020-13-01')) && isNaN(Date.parse('nope')) && isNaN(Date.parse(''))",
        "const s = new Date(1582979415000); Date.parse(s.toString()) === 1582979415000",
        "Date.parse(s.toUTCString()) === 1582979415000 && Date.parse(s.toISOString()) === 1582979415000",
        "Date.parse('Sat, 29 Feb 2020 12:30:15 GMT') === Date.UTC(2020, 1, 29, 12, 30, 15)",
        "Date.parse('Thu, 01 Jan 1970 00:00:00 GMT+0100') === -3600000",
        "Date.parse('Feb 29 2020') === new Date(2020, 1, 29).getTime()",
        "Date.parse('February 29, 2020 10:20:30') === new Date(2020, 1, 29, 10, 20, 30).getTime()",
        "Date.parse('2020/02/29') === new Date(2020, 1, 29).getTime()",
        "Date.parse('2/29/2020') === new Date(2020, 1, 29).getTime()",
        "Date.parse('29 Feb 2020 10:00 UTC') === Date.UTC(2020, 1, 29, 10)",
    ]);
}

#[test]
fn number_format_groups_by_locale() {
    expect_strings(&[
        (
            "new Intl.NumberFormat('en-US').format(1234567.891)",
            "1,234,567.891",
        ),
        (
            "new Intl.NumberFormat('pt-BR').format(1234567.891)",
            "1.234.567,891",
        ),
        ("(1234.5).toLocaleString('pt-BR')", "1.234,5"),
        ("(1234.5).toLocaleString()", "1,234.5"),
        ("(12345n).toLocaleString('en')", "12,345"),
        (
            "new Intl.NumberFormat('en', { minimumFractionDigits: 2 }).format(1)",
            "1.00",
        ),
        (
            "new Intl.NumberFormat('en', { maximumFractionDigits: 0 }).format(2.5)",
            "3",
        ),
        (
            "new Intl.NumberFormat('en', { useGrouping: false }).format(12345)",
            "12345",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'percent' }).format(0.256)",
            "26%",
        ),
        ("new Intl.NumberFormat('en').format(-0.5)", "-0.5"),
        ("new Intl.NumberFormat('en').format(NaN)", "NaN"),
        ("new Intl.NumberFormat('en').format(-Infinity)", "-∞"),
        (
            "new Intl.NumberFormat('pt-BR').resolvedOptions().locale",
            "pt-BR",
        ),
        (
            "new Intl.NumberFormat('en').formatToParts(1234.5).map(p => p.type).join()",
            "integer,group,integer,decimal,fraction",
        ),
        ("[1234, 5678].toLocaleString('pt-BR')", "1.234,5.678"),
    ]);
}

#[test]
fn plural_rules_select_cardinal_and_ordinal_categories() {
    expect_strings(&[
        ("new Intl.PluralRules('en').select(1)", "one"),
        ("new Intl.PluralRules('en').select(2)", "other"),
        (
            "new Intl.PluralRules('en', { type: 'ordinal' }).select(2)",
            "two",
        ),
        (
            "new Intl.PluralRules('en', { type: 'ordinal' }).select(23)",
            "few",
        ),
        ("new Intl.PluralRules('pt').select(0)", "one"),
        ("new Intl.PluralRules('pt').select(5)", "other"),
        (
            "new Intl.PluralRules('en').resolvedOptions().pluralCategories.join()",
            "one,other",
        ),
    ]);
}

#[test]
fn collator_compares_by_locale_and_options() {
    expect_strings(&[
        (
            "['b', 'z', 'ä', 'a'].sort(new Intl.Collator('de').compare).join()",
            "a,ä,b,z",
        ),
        (
            "['10', '9', '2'].sort(new Intl.Collator('en', { numeric: true }).compare).join()",
            "2,9,10",
        ),
        ("new Intl.Collator('pt').resolvedOptions().locale", "pt"),
    ]);
    expect_true(&[
        "'a'.localeCompare('B') < 0 && 'b'.localeCompare('A') > 0 && 'a'.localeCompare('a') === 0",
        "new Intl.Collator('en', { sensitivity: 'base' }).compare('a', 'A') === 0",
        "new Intl.Collator('en', { sensitivity: 'base' }).compare('a', 'á') === 0",
        "new Intl.Collator('en').compare('a', 'á') < 0",
        "'résumé'.localeCompare('resume', 'en', { sensitivity: 'base' }) === 0",
    ]);
}

#[test]
fn date_time_format_follows_cldr_patterns() {
    // V8 (CLDR 42+) puts U+202F before the day period; other spaces stay plain.
    let nnbsp = |s: &str| s.replace(" AM", "\u{202f}AM");
    expect_strings(&[
        (
            "new Intl.DateTimeFormat('en-US', { timeZone: 'UTC' }).format(new Date(0))",
            "1/1/1970",
        ),
        (
            "new Intl.DateTimeFormat('pt-BR', { timeZone: 'UTC' }).format(new Date(0))",
            "01/01/1970",
        ),
        (
            "new Intl.DateTimeFormat('en-US', { dateStyle: 'full', timeZone: 'UTC' }).format(0)",
            "Thursday, January 1, 1970",
        ),
        (
            "new Intl.DateTimeFormat('pt-BR', { dateStyle: 'long', timeZone: 'UTC' }).format(0)",
            "1 de janeiro de 1970",
        ),
        (
            "new Date(0).toLocaleDateString('pt-BR', { timeZone: 'UTC' })",
            "01/01/1970",
        ),
        (
            "new Date(Date.UTC(2020, 5, 1, 15)).toLocaleTimeString('pt-BR', { timeZone: 'UTC' })",
            "15:00:00",
        ),
        (
            "new Intl.DateTimeFormat('pt-BR', { timeZone: 'America/Sao_Paulo', timeStyle: 'short' }).format(Date.UTC(2020, 5, 1, 15))",
            "12:00",
        ),
        (
            "new Intl.DateTimeFormat('en-US', { timeZone: 'Asia/Tokyo' }).format(Date.UTC(2020, 11, 31, 20))",
            "1/1/2021",
        ),
        (
            "new Intl.DateTimeFormat('en-US', { timeZone: 'UTC' }).resolvedOptions().timeZone",
            "UTC",
        ),
    ]);
    expect_strings(&[
        (
            "new Intl.DateTimeFormat('en-US', { timeStyle: 'short', timeZone: 'UTC' }).format(0)",
            &nnbsp("12:00 AM"),
        ),
        (
            "new Date(0).toLocaleString('en-US', { timeZone: 'UTC' })",
            &nnbsp("1/1/1970, 12:00:00 AM"),
        ),
        (
            "new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', hour: 'numeric', minute: '2-digit' }).formatToParts(0).map(p => p.type + '=' + p.value).join('|')",
            &nnbsp("hour=12|literal=:|minute=00|literal= AM").replace("AM", "|dayPeriod=AM"),
        ),
    ]);
    expect_true(&[
        "typeof new Date(0).toLocaleString() === 'string' && new Date(NaN).toLocaleString() === 'Invalid Date'",
        "(() => { try { new Intl.DateTimeFormat('en', { timeZone: 'Mars/Base' }); } catch (e) { return e instanceof RangeError; } })()",
        "(() => { try { new Intl.DateTimeFormat('en').format(NaN); } catch (e) { return e instanceof RangeError; } })()",
    ]);
}

#[test]
fn segmenter_splits_graphemes_and_words() {
    expect_strings(&[
        (
            "[...new Intl.Segmenter('en', { granularity: 'word' }).segment('Hello, world')].filter(s => s.isWordLike).map(s => s.segment).join('|')",
            "Hello|world",
        ),
        (
            "[...new Intl.Segmenter('en').segment('a👨‍👩‍👧b')].map(s => s.index).join()",
            "0,1,9",
        ),
    ]);
}

#[test]
fn intl_namespace_and_locale_negotiation() {
    expect_strings(&[
        ("Object.prototype.toString.call(Intl)", "[object Intl]"),
        ("Intl.getCanonicalLocales('EN-us').join()", "en-US"),
        (
            "Intl.NumberFormat.supportedLocalesOf(['pt-BR', 'en']).join()",
            "pt-BR,en",
        ),
        (
            "Object.prototype.toString.call(new Intl.Collator('en'))",
            "[object Intl.Collator]",
        ),
    ]);
    expect_true(&[
        "(() => { try { Intl.getCanonicalLocales('not a locale!'); } catch (e) { return e instanceof RangeError; } })()",
        "(() => { try { Intl.NumberFormat.prototype.format.call({}); } catch (e) { return e instanceof TypeError; } })()",
    ]);
}

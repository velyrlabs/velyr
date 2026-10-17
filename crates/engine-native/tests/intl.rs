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

const NB: &str = "\u{a0}";

fn number(locale: &str, options: &str, value: &str) -> String {
    format!("new Intl.NumberFormat('{locale}', {options}).format({value})")
}

#[test]
fn number_format_significant_digits() {
    expect_strings(&[
        (
            &number("en-US", "{ maximumSignificantDigits: 3 }", "123456"),
            "123,000",
        ),
        (
            &number("en-US", "{ minimumSignificantDigits: 5 }", "1.5"),
            "1.5000",
        ),
        (
            &number("en-US", "{ maximumSignificantDigits: 2 }", "0.012345"),
            "0.012",
        ),
        (
            &number("pt-BR", "{ maximumSignificantDigits: 3 }", "1234.5678"),
            "1.230",
        ),
        (
            &number(
                "en-US",
                "{ maximumSignificantDigits: 3, minimumFractionDigits: 1 }",
                "1.2345",
            ),
            "1.23",
        ),
        (
            &number("en-US", "{ maximumSignificantDigits: 3 }", "999.96"),
            "1,000",
        ),
        (
            &number("en-US", "{ minimumSignificantDigits: 3 }", "0"),
            "0.00",
        ),
        (
            &number(
                "en-US",
                "{ maximumSignificantDigits: 3, style: 'percent' }",
                "0.12345",
            ),
            "12.3%",
        ),
        (
            "JSON.stringify(new Intl.NumberFormat('en', { maximumSignificantDigits: 3 }).resolvedOptions().minimumSignificantDigits)",
            "1",
        ),
    ]);
    expect_true(&[
        "(() => { try { new Intl.NumberFormat('en', { maximumSignificantDigits: 0 }); } catch (e) { return e instanceof RangeError; } })()",
        "(() => { try { new Intl.NumberFormat('en', { minimumSignificantDigits: 5, maximumSignificantDigits: 2 }); } catch (e) { return e instanceof RangeError; } })()",
    ]);
}

#[test]
fn number_format_percent_and_sign_display() {
    expect_strings(&[
        (&number("pt-BR", "{ style: 'percent' }", "0.256"), "26%"),
        (
            &number("de-DE", "{ style: 'percent' }", "0.256"),
            &format!("26{NB}%"),
        ),
        (
            &number(
                "en-US",
                "{ style: 'percent', minimumFractionDigits: 1 }",
                "0.256",
            ),
            "25.6%",
        ),
        (
            &number(
                "en-US",
                "{ style: 'percent', signDisplay: 'always' }",
                "0.5",
            ),
            "+50%",
        ),
        (&number("en-US", "{ signDisplay: 'always' }", "5"), "+5"),
        (&number("en-US", "{ signDisplay: 'never' }", "-5"), "5"),
        (&number("en-US", "{ signDisplay: 'exceptZero' }", "0"), "0"),
        (&number("en-US", "{ signDisplay: 'exceptZero' }", "5"), "+5"),
        (
            "new Intl.NumberFormat('de-DE', { style: 'percent' }).formatToParts(0.5).map(p => p.type).join()",
            "integer,literal,percentSign",
        ),
    ]);
}

#[test]
fn number_format_currency() {
    expect_strings(&[
        (
            &number("en-US", "{ style: 'currency', currency: 'USD' }", "1234.5"),
            "$1,234.50",
        ),
        (
            &number("en-US", "{ style: 'currency', currency: 'usd' }", "-1234.5"),
            "-$1,234.50",
        ),
        (
            &number("en-US", "{ style: 'currency', currency: 'EUR' }", "-1234.5"),
            "-\u{20ac}1,234.50",
        ),
        (
            &number("en-US", "{ style: 'currency', currency: 'JPY' }", "1234.5"),
            "\u{a5}1,235",
        ),
        (
            &number("en-US", "{ style: 'currency', currency: 'KWD' }", "1.5"),
            &format!("KWD{NB}1.500"),
        ),
        (
            &number("en-US", "{ style: 'currency', currency: 'CAD' }", "1"),
            "CA$1.00",
        ),
        (
            &number("pt-BR", "{ style: 'currency', currency: 'BRL' }", "1234.5"),
            &format!("R${NB}1.234,50"),
        ),
        (
            &number("pt-BR", "{ style: 'currency', currency: 'BRL' }", "-1234.5"),
            &format!("-R${NB}1.234,50"),
        ),
        (
            &number("pt-BR", "{ style: 'currency', currency: 'USD' }", "1234.5"),
            &format!("US${NB}1.234,50"),
        ),
        (
            &number("de-DE", "{ style: 'currency', currency: 'EUR' }", "1234.5"),
            &format!("1.234,50{NB}\u{20ac}"),
        ),
        (
            &number(
                "en-US",
                "{ style: 'currency', currency: 'USD', currencyDisplay: 'code' }",
                "1234.5",
            ),
            &format!("USD{NB}1,234.50"),
        ),
        (
            &number(
                "en-US",
                "{ style: 'currency', currency: 'CAD', currencyDisplay: 'narrowSymbol' }",
                "1",
            ),
            "$1.00",
        ),
        (
            &number(
                "en-US",
                "{ style: 'currency', currency: 'USD', currencySign: 'accounting' }",
                "-1234.5",
            ),
            "($1,234.50)",
        ),
        (
            &number(
                "en-US",
                "{ style: 'currency', currency: 'USD', minimumFractionDigits: 0 }",
                "1234",
            ),
            "$1,234",
        ),
        (
            &number(
                "en-US",
                "{ style: 'currency', currency: 'USD', maximumFractionDigits: 0 }",
                "1234.5",
            ),
            "$1,235",
        ),
        (
            "new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' }).formatToParts(-1234.5).map(p => p.type).join()",
            "minusSign,currency,integer,group,integer,decimal,fraction",
        ),
        (
            "JSON.stringify(new Intl.NumberFormat('en-US', { style: 'currency', currency: 'usd' }).resolvedOptions())",
            "{\"locale\":\"en-US\",\"numberingSystem\":\"latn\",\"style\":\"currency\",\"currency\":\"USD\",\"currencyDisplay\":\"symbol\",\"currencySign\":\"standard\",\"minimumIntegerDigits\":1,\"minimumFractionDigits\":2,\"maximumFractionDigits\":2,\"useGrouping\":\"auto\",\"notation\":\"standard\",\"signDisplay\":\"auto\",\"roundingIncrement\":1,\"roundingMode\":\"halfExpand\",\"roundingPriority\":\"auto\",\"trailingZeroDisplay\":\"auto\"}",
        ),
    ]);
    expect_true(&[
        "(() => { try { new Intl.NumberFormat('en', { style: 'currency' }); } catch (e) { return e instanceof TypeError; } })()",
        "(() => { try { new Intl.NumberFormat('en', { style: 'currency', currency: 'US' }); } catch (e) { return e instanceof RangeError; } })()",
    ]);
}

#[test]
fn number_format_scientific_and_engineering() {
    expect_strings(&[
        (
            &number("en-US", "{ notation: 'scientific' }", "123456"),
            "1.235E5",
        ),
        (
            &number("en-US", "{ notation: 'scientific' }", "-0.000123456"),
            "-1.235E-4",
        ),
        (&number("en-US", "{ notation: 'scientific' }", "0"), "0E0"),
        (
            &number(
                "en-US",
                "{ notation: 'scientific', maximumFractionDigits: 1 }",
                "9999",
            ),
            "1E4",
        ),
        (
            &number(
                "en-US",
                "{ notation: 'scientific', maximumSignificantDigits: 2 }",
                "123456",
            ),
            "1.2E5",
        ),
        (
            &number("en-US", "{ notation: 'engineering' }", "123456"),
            "123.456E3",
        ),
        (
            &number("en-US", "{ notation: 'engineering' }", "12345678"),
            "12.346E6",
        ),
        (
            &number("en-US", "{ notation: 'engineering' }", "0.00123"),
            "1.23E-3",
        ),
        (
            &number("pt-BR", "{ notation: 'scientific' }", "0.00123"),
            "1,23E-3",
        ),
        (
            &number("pt-BR", "{ notation: 'engineering' }", "123456"),
            "123,456E3",
        ),
        (
            "new Intl.NumberFormat('en-US', { notation: 'scientific' }).formatToParts(0.00123).map(p => p.type).join()",
            "integer,decimal,fraction,exponentSeparator,exponentMinusSign,exponentInteger",
        ),
    ]);
}

/// `new Intl.DateTimeFormat(locale, { timeZone, ...options }).format(2024-01-05T15:04:09Z)`.
fn date_time(locale: &str, zone: &str, options: &str) -> String {
    format!(
        "new Intl.DateTimeFormat('{locale}', {{ timeZone: '{zone}', {options} }}).format(Date.UTC(2024, 0, 5, 15, 4, 9))"
    )
}

/// V8 (CLDR 42+) puts U+202F before the day period.
fn nnbsp(s: &str) -> String {
    s.replace(" AM", "\u{202f}AM").replace(" PM", "\u{202f}PM")
}

#[test]
fn date_time_format_two_digit_and_numeric_fields() {
    let all = "year: '2-digit', month: '2-digit', day: '2-digit'";
    let numeric = "year: 'numeric', month: 'numeric', day: 'numeric'";
    expect_strings(&[
        (&date_time("en-US", "UTC", all), "01/05/24"),
        (&date_time("en-US", "UTC", numeric), "1/5/2024"),
        (
            &date_time("en-US", "UTC", "month: '2-digit', day: '2-digit'"),
            "01/05",
        ),
        (
            &date_time("en-US", "UTC", "hour: '2-digit', minute: '2-digit'"),
            &nnbsp("03:04 PM"),
        ),
        (&date_time("pt-BR", "UTC", all), "05/01/24"),
        (&date_time("pt-BR", "UTC", numeric), "05/01/2024"),
        (
            &date_time("pt-BR", "UTC", "month: '2-digit', day: '2-digit'"),
            "05/01",
        ),
        (
            &date_time("pt-BR", "UTC", "hour: '2-digit', minute: '2-digit'"),
            "15:04",
        ),
        (
            &date_time(
                "en-US",
                "UTC",
                "hour: 'numeric', minute: '2-digit', second: '2-digit', hour12: false",
            ),
            "15:04:09",
        ),
        (
            &date_time(
                "en-US",
                "UTC",
                "weekday: 'long', month: 'short', day: 'numeric'",
            ),
            "Friday, Jan 5",
        ),
        (
            &date_time(
                "pt-BR",
                "UTC",
                "weekday: 'long', month: 'short', day: 'numeric'",
            ),
            "sexta-feira, 5 de jan.",
        ),
    ]);
}

#[test]
fn date_time_format_style_lengths_differ() {
    let sp = "America/Sao_Paulo";
    expect_strings(&[
        (
            &date_time("en-US", "UTC", "dateStyle: 'full', timeStyle: 'short'"),
            &nnbsp("Friday, January 5, 2024 at 3:04 PM"),
        ),
        (
            &date_time("en-US", "UTC", "dateStyle: 'short', timeStyle: 'full'"),
            &nnbsp("1/5/24, 3:04:09 PM Coordinated Universal Time"),
        ),
        (
            &date_time("en-US", sp, "dateStyle: 'medium', timeStyle: 'long'"),
            &nnbsp("Jan 5, 2024, 12:04:09 PM GMT-3"),
        ),
        (
            &date_time("en-US", sp, "dateStyle: 'long', timeStyle: 'medium'"),
            &nnbsp("January 5, 2024 at 12:04:09 PM"),
        ),
        (
            &date_time("en-US", sp, "timeStyle: 'full'"),
            &nnbsp("12:04:09 PM Brasilia Standard Time"),
        ),
        (
            &date_time("pt-BR", "UTC", "dateStyle: 'full', timeStyle: 'short'"),
            "sexta-feira, 5 de janeiro de 2024 \u{e0}s 15:04",
        ),
        (
            &date_time("pt-BR", "UTC", "dateStyle: 'short', timeStyle: 'full'"),
            "05/01/2024, 15:04:09 Hor\u{e1}rio Universal Coordenado",
        ),
        (
            &date_time("pt-BR", sp, "dateStyle: 'medium', timeStyle: 'long'"),
            "5 de jan. de 2024, 12:04:09 BRT",
        ),
        (
            &date_time("pt-BR", sp, "timeStyle: 'full'"),
            "12:04:09 Hor\u{e1}rio Padr\u{e3}o de Bras\u{ed}lia",
        ),
    ]);
}

#[test]
fn date_time_format_time_zone_name() {
    let sp = "America/Sao_Paulo";
    let ny = "America/New_York";
    let hour = |name: &str| format!("hour: 'numeric', timeZoneName: '{name}'");
    expect_strings(&[
        (
            &date_time("en-US", sp, &hour("short")),
            &nnbsp("12 PM GMT-3"),
        ),
        (
            &date_time("en-US", sp, &hour("long")),
            &nnbsp("12 PM Brasilia Standard Time"),
        ),
        (
            &date_time("en-US", sp, &hour("shortOffset")),
            &nnbsp("12 PM GMT-3"),
        ),
        (
            &date_time("en-US", sp, &hour("longOffset")),
            &nnbsp("12 PM GMT-03:00"),
        ),
        (&date_time("en-US", ny, &hour("short")), &nnbsp("10 AM EST")),
        (
            &date_time("en-US", ny, &hour("shortGeneric")),
            &nnbsp("10 AM ET"),
        ),
        (
            &date_time("en-US", ny, &hour("longGeneric")),
            &nnbsp("10 AM Eastern Time"),
        ),
        (&date_time("pt-BR", sp, &hour("short")), "12 BRT"),
        (
            &date_time("pt-BR", sp, &hour("long")),
            "12 Hor\u{e1}rio Padr\u{e3}o de Bras\u{ed}lia",
        ),
        (&date_time("pt-BR", ny, &hour("short")), "10 GMT-5"),
        (
            "new Intl.DateTimeFormat('en-US', { timeZone: 'America/New_York', hour: 'numeric', timeZoneName: 'short' }).formatToParts(0).map(p => p.type).join()",
            "hour,literal,dayPeriod,literal,timeZoneName",
        ),
        (
            "new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', timeZoneName: 'short' }).resolvedOptions().timeZoneName",
            "short",
        ),
    ]);
}

#[test]
fn locale_extensions_select_calendar_numbering_and_hour_cycle() {
    let utc = "UTC";
    expect_strings(&[
        (&date_time("en-US-u-ca-buddhist", utc, ""), "1/5/2567 BE"),
        (&date_time("en-US-u-ca-japanese", utc, ""), "1/5/6 R"),
        (
            &date_time("en-US", utc, "calendar: 'buddhist'"),
            "1/5/2567 BE",
        ),
        (
            &date_time("en-US-u-nu-arab", utc, ""),
            "\u{661}/\u{665}/\u{662}\u{660}\u{662}\u{664}",
        ),
        (
            &date_time("en-US", utc, "numberingSystem: 'arab'"),
            "\u{661}/\u{665}/\u{662}\u{660}\u{662}\u{664}",
        ),
        (
            &date_time("en-US-u-hc-h23", utc, "timeStyle: 'short'"),
            "15:04",
        ),
        (
            &date_time("en-US", utc, "hourCycle: 'h23', timeStyle: 'short'"),
            "15:04",
        ),
        (
            "new Intl.DateTimeFormat('en-US-u-ca-buddhist-nu-arab-hc-h23', { timeZone: 'UTC' }).resolvedOptions().locale",
            "en-US-u-ca-buddhist-hc-h23-nu-arab",
        ),
        (
            "new Intl.DateTimeFormat('en-US-u-ca-buddhist', { timeZone: 'UTC', calendar: 'gregory' }).resolvedOptions().locale",
            "en-US",
        ),
        (
            "new Intl.DateTimeFormat('en-US-u-ca-buddhist-nu-arab', { timeZone: 'UTC' }).resolvedOptions().calendar + '/' + new Intl.DateTimeFormat('en-US-u-ca-buddhist-nu-arab').resolvedOptions().numberingSystem",
            "buddhist/arab",
        ),
        (
            "new Intl.NumberFormat('en-US-u-nu-arab').format(25)",
            "\u{662}\u{665}",
        ),
        (
            "new Intl.NumberFormat('en-US', { numberingSystem: 'deva' }).format(1234.5)",
            "\u{967},\u{968}\u{969}\u{96a}.\u{96b}",
        ),
        (
            "new Intl.NumberFormat('pt-BR-u-nu-thai').format(1234.5)",
            "\u{e51}.\u{e52}\u{e53}\u{e54},\u{e55}",
        ),
        (
            "new Intl.NumberFormat('en-US-u-nu-arab').resolvedOptions().locale",
            "en-US-u-nu-arab",
        ),
        (
            "new Intl.NumberFormat('en-US-u-nu-arab', { numberingSystem: 'latn' }).resolvedOptions().locale",
            "en-US",
        ),
        (
            "new Intl.NumberFormat('en-US-u-ca-buddhist').resolvedOptions().locale",
            "en-US",
        ),
    ]);
}

#[test]
fn prelude_functions_print_as_native_code() {
    expect_strings(&[
        (
            "Intl.NumberFormat.prototype.formatToParts.toString()",
            "function formatToParts() { [native code] }",
        ),
        (
            "Intl.NumberFormat.toString()",
            "function NumberFormat() { [native code] }",
        ),
        (
            "Date.prototype.toLocaleString.toString()",
            "function toLocaleString() { [native code] }",
        ),
        (
            "(new Intl.Collator('en').compare).toString()",
            "function () { [native code] }",
        ),
    ]);
    expect_true(&[
        "Object.getOwnPropertyDescriptor(Intl.DateTimeFormat.prototype, 'format').get.toString().endsWith('{ [native code] }')",
    ]);
}

#[test]
fn date_to_string_names_the_local_zone() {
    expect_true(&[
        "/^\\w{3} \\w{3} \\d{2} \\d{4} \\d{2}:\\d{2}:\\d{2} GMT[+-]\\d{4} \\(.+\\)$/.test(new Date(1704467049000).toString())",
        "new Date(1704467049000).toString().endsWith('(' + new Intl.DateTimeFormat('en-US', { timeZoneName: 'long' }).formatToParts(1704467049000).find(p => p.type === 'timeZoneName').value + ')')",
        "new Date(1704467049000).toTimeString().includes(' (')",
    ]);
}

#[test]
fn intl_constructors_leave_regexp_legacy_statics_untouched() {
    expect_true(&[
        "new Intl.Collator('de-u-co-phonebk', { numeric: true }); RegExp.$1 === ''",
        "new Intl.DateTimeFormat('en-u-ca-gregory', { calendar: 'gregory' }); RegExp.$1 === ''",
        "new Intl.NumberFormat('en-u-nu-latn', { currency: 'usd' }); RegExp.$1 === ''",
        "new Intl.DateTimeFormat('en-u-ca-buddhist').resolvedOptions().calendar === 'buddhist'",
    ]);
}

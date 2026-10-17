//! `Intl.NumberFormat` v3 options, `formatRange` and `PluralRules.selectRange` (en and pt).

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
            name: "intl_number_v3.js".into(),
            text: (*code).into(),
        };
        match iso.eval_script(&source) {
            Ok(value) => assert_eq!(value, Value::Bool(true), "{code}"),
            Err(error) => panic!("`{code}` threw {}", error.message),
        }
    }
}

const THROWS: &str =
    "(C, f) => { try { f(); } catch (e) { return e instanceof C; } return false; }";

fn throws(kind: &str, body: &str) -> String {
    format!("({THROWS})({kind}, () => {{ {body} }})")
}

#[test]
fn rounding_modes_and_increments() {
    expect_true(&[
        "new Intl.NumberFormat('en',{maximumFractionDigits:0,roundingMode:'floor'}).format(2.9) === '2'",
        "new Intl.NumberFormat('en',{maximumFractionDigits:0,roundingMode:'ceil'}).format(-2.1) === '-2'",
        "new Intl.NumberFormat('en',{maximumFractionDigits:0,roundingMode:'halfEven'}).format(2.5) === '2'",
        "new Intl.NumberFormat('en',{maximumFractionDigits:0,roundingMode:'halfCeil'}).format(-2.5) === '-2'",
        "new Intl.NumberFormat('en',{maximumFractionDigits:0,roundingMode:'halfFloor'}).format(2.5) === '2'",
        "new Intl.NumberFormat('en',{maximumFractionDigits:0,roundingMode:'halfTrunc'}).format(-2.5) === '-2'",
        "new Intl.NumberFormat('en',{maximumFractionDigits:0,roundingMode:'expand'}).format(2.1) === '3'",
        "new Intl.NumberFormat('en',{maximumFractionDigits:0,roundingMode:'trunc'}).format(2.9) === '2'",
        "new Intl.NumberFormat('en',{roundingIncrement:5,maximumFractionDigits:2,minimumFractionDigits:2}).format(1.07) === '1.05'",
        "new Intl.NumberFormat('en',{roundingIncrement:250,maximumFractionDigits:3,minimumFractionDigits:3}).format(1.3) === '1.250'",
        "new Intl.NumberFormat('en',{roundingIncrement:10,maximumFractionDigits:1,minimumFractionDigits:1}).format(7) === '7.0'",
        "new Intl.NumberFormat('en',{roundingIncrement:5}).resolvedOptions().maximumFractionDigits === 0",
    ]);
}

#[test]
fn priority_and_trailing_zeros() {
    expect_true(&[
        "new Intl.NumberFormat('en',{trailingZeroDisplay:'stripIfInteger',minimumFractionDigits:2}).format(3) === '3'",
        "new Intl.NumberFormat('en',{trailingZeroDisplay:'stripIfInteger',minimumFractionDigits:2}).format(3.5) === '3.50'",
        "new Intl.NumberFormat('en',{roundingPriority:'morePrecision',maximumFractionDigits:2,maximumSignificantDigits:2}).format(1.2345) === '1.23'",
        "new Intl.NumberFormat('en',{roundingPriority:'lessPrecision',maximumFractionDigits:2,maximumSignificantDigits:2}).format(1.2345) === '1.2'",
        "new Intl.NumberFormat('en',{roundingPriority:'lessPrecision',maximumFractionDigits:2,maximumSignificantDigits:2}).resolvedOptions().roundingPriority === 'lessPrecision'",
    ]);
}

#[test]
fn grouping_and_exact_decimals() {
    expect_true(&[
        "new Intl.NumberFormat('en',{useGrouping:'min2'}).format(1234) === '1234'",
        "new Intl.NumberFormat('en',{useGrouping:'min2'}).format(12345) === '12,345'",
        "new Intl.NumberFormat('en',{useGrouping:'always'}).format(1234) === '1,234'",
        "new Intl.NumberFormat('en',{useGrouping:false}).format(1234) === '1234'",
        "new Intl.NumberFormat('en',{useGrouping:'false'}).resolvedOptions().useGrouping === 'auto'",
        "new Intl.NumberFormat('en').resolvedOptions().useGrouping === 'auto'",
        "new Intl.NumberFormat('en').format('9007199254740993') === '9,007,199,254,740,993'",
        "new Intl.NumberFormat('en',{maximumFractionDigits:20}).format('1234.567890123456789') === '1,234.567890123456789'",
        "new Intl.NumberFormat('en').format(' 12 ') === '12'",
    ]);
}

#[test]
fn format_range() {
    expect_true(&[
        "new Intl.NumberFormat('en').formatRange(3, 5) === '3\u{2013}5'",
        "new Intl.NumberFormat('en').formatRange(3, 3) === '~3'",
        "new Intl.NumberFormat('en',{maximumFractionDigits:0}).formatRange(2.9, 3.1) === '~3'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'USD',maximumFractionDigits:0}).formatRange(3, 5) === '$3 \u{2013} $5'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'USD',signDisplay:'always'}).formatRange(2.9, 3.1) === '+$2.90\u{2013}3.10'",
        "new Intl.NumberFormat('en').formatRange('987654321987654321', '987654321987654322') === '987,654,321,987,654,321\u{2013}987,654,321,987,654,322'",
        "new Intl.NumberFormat('en').formatRange(23, 12) === '23\u{2013}12'",
        "new Intl.NumberFormat('pt-PT',{style:'currency',currency:'EUR',maximumFractionDigits:0}).formatRange(3, 5) === '3 - 5\u{a0}\u{20ac}'",
        "JSON.stringify(new Intl.NumberFormat('en').formatRangeToParts(3, 5)) === '[{\"type\":\"integer\",\"value\":\"3\",\"source\":\"startRange\"},{\"type\":\"literal\",\"value\":\"\u{2013}\",\"source\":\"shared\"},{\"type\":\"integer\",\"value\":\"5\",\"source\":\"endRange\"}]'",
        "JSON.stringify(new Intl.NumberFormat('en').formatRangeToParts(1, 1)) === '[{\"type\":\"approximatelySign\",\"value\":\"~\",\"source\":\"shared\"},{\"type\":\"integer\",\"value\":\"1\",\"source\":\"shared\"}]'",
        "Intl.NumberFormat.prototype.formatRange.length === 2",
        &throws(
            "RangeError",
            "new Intl.NumberFormat('en').formatRange(NaN, 1)",
        ),
        &throws("TypeError", "new Intl.NumberFormat('en').formatRange(1)"),
    ]);
}

#[test]
fn plural_rules_v3() {
    expect_true(&[
        "new Intl.PluralRules('en').selectRange(1, 2) === 'other'",
        "new Intl.PluralRules('en').selectRange(1, 1) === 'one'",
        "new Intl.PluralRules('en',{minimumFractionDigits:1}).select(1) === 'other'",
        "new Intl.PluralRules('en',{maximumFractionDigits:0,roundingMode:'floor'}).select(1.9) === 'one'",
        "new Intl.PluralRules('en',{roundingIncrement:5,maximumFractionDigits:1,minimumFractionDigits:1}).resolvedOptions().roundingIncrement === 5",
        "new Intl.PluralRules('en').resolvedOptions().roundingMode === 'halfExpand'",
        "new Intl.PluralRules('en').resolvedOptions().trailingZeroDisplay === 'auto'",
        "Intl.PluralRules.prototype.selectRange.length === 2",
        &throws(
            "RangeError",
            "new Intl.PluralRules('en').selectRange(NaN, 1)",
        ),
        &throws("TypeError", "new Intl.PluralRules('en').selectRange(1)"),
    ]);
}

#[test]
fn rejects_bad_options() {
    expect_true(&[
        &throws(
            "RangeError",
            "new Intl.NumberFormat('en',{roundingMode:'x'})",
        ),
        &throws(
            "RangeError",
            "new Intl.NumberFormat('en',{roundingIncrement:3})",
        ),
        &throws(
            "RangeError",
            "new Intl.NumberFormat('en',{roundingIncrement:5000.5})",
        ),
        &throws(
            "RangeError",
            "new Intl.NumberFormat('en',{roundingIncrement:5,minimumFractionDigits:0,maximumFractionDigits:2})",
        ),
        &throws(
            "TypeError",
            "new Intl.NumberFormat('en',{roundingIncrement:5,maximumSignificantDigits:3})",
        ),
        &throws(
            "RangeError",
            "new Intl.NumberFormat('en',{trailingZeroDisplay:'x'})",
        ),
        &throws(
            "RangeError",
            "new Intl.NumberFormat('en',{roundingPriority:'x'})",
        ),
        &throws(
            "RangeError",
            "new Intl.NumberFormat('en',{useGrouping:'x'})",
        ),
    ]);
}

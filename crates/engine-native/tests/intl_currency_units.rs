//! `Intl.NumberFormat` currency display names, accounting sign and `style:'unit'`.

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
            name: "intl_currency_units.js".into(),
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
fn currency_accounting_and_names() {
    expect_true(&[
        "new Intl.NumberFormat('en',{style:'currency',currency:'USD',currencySign:'accounting'}).format(-3) === '($3.00)'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'EUR',currencyDisplay:'name'}).format(1) === '1.00 euros'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'USD',currencyDisplay:'name',minimumFractionDigits:0}).format(1) === '1 US dollar'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'USD',currencyDisplay:'name'}).format(-2.5) === '-2.50 US dollars'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'JPY',currencyDisplay:'name'}).format(5) === '5 Japanese yen'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'XTS',currencyDisplay:'name'}).format(5) === '5.00 XTS'",
        "new Intl.NumberFormat('pt-BR',{style:'currency',currency:'BRL',currencyDisplay:'name'}).format(2) === '2,00 reais brasileiros'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'EUR',currencyDisplay:'name'}).formatToParts(2).map(p => p.type).join() === 'integer,decimal,fraction,literal,currency'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'RUB',currencyDisplay:'narrowSymbol'}).format(1) === '\u{20bd}1.00'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'THB',currencyDisplay:'narrowSymbol'}).format(1) === '\u{e3f}1.00'",
        "new Intl.NumberFormat('en',{style:'currency',currency:'XAF'}).format(1) === 'FCFA\u{a0}1'",
    ]);
}

#[test]
fn unit_style_en() {
    expect_true(&[
        "new Intl.NumberFormat('en',{style:'unit',unit:'kilometer-per-hour',unitDisplay:'long'}).format(2) === '2 kilometers per hour'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'kilometer-per-hour'}).format(987) === '987 km/h'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'kilometer-per-hour',unitDisplay:'narrow'}).format(-987) === '-987km/h'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'meter',unitDisplay:'long'}).format(1) === '1 meter'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'meter',unitDisplay:'long'}).format(1.5) === '1.5 meters'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'celsius'}).format(21) === '21\u{b0}C'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'percent'}).format(50) === '50%'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'gigabyte',unitDisplay:'long'}).format(3) === '3 gigabytes'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'second',unitDisplay:'long'}).format(1) === '1 second'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'liter-per-kilometer',unitDisplay:'long'}).format(2) === '2 liters per kilometer'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'byte-per-second'}).format(2) === '2 byte/sec'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'meter'}).formatToParts(-2.5).map(p => p.type).join() === 'minusSign,integer,decimal,fraction,literal,unit'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'meter',unitDisplay:'narrow'}).formatToParts(2).map(p => p.type).join() === 'integer,unit'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'mile',maximumFractionDigits:0}).formatRange(3,5) === '3\u{2013}5 mi'",
        "new Intl.NumberFormat('en',{style:'unit',unit:'foot'}).resolvedOptions().unitDisplay === 'short'",
        "Object.keys(new Intl.NumberFormat('en',{style:'unit',unit:'foot'}).resolvedOptions()).slice(0,6).join() === 'locale,numberingSystem,style,unit,unitDisplay,minimumIntegerDigits'",
    ]);
}

#[test]
fn unit_style_other_locales() {
    expect_true(&[
        "new Intl.NumberFormat('pt',{style:'unit',unit:'kilometer',unitDisplay:'long'}).format(2) === '2 quil\u{f4}metros'",
        "new Intl.NumberFormat('pt',{style:'unit',unit:'kilometer-per-hour',unitDisplay:'long'}).format(1) === '1 quil\u{f4}metro por hora'",
        "new Intl.NumberFormat('pt',{style:'unit',unit:'meter'}).format(1.5) === '1,5 m'",
        "new Intl.NumberFormat('xx',{style:'unit',unit:'meter',unitDisplay:'long'}).format(2) === '2 meters'",
    ]);
}

#[test]
#[cfg(feature = "intl-all-locales")]
fn unit_style_cldr_locales() {
    expect_true(&[
        "new Intl.NumberFormat('de',{style:'unit',unit:'kilometer-per-hour',unitDisplay:'long'}).format(987) === '987 Kilometer pro Stunde'",
        "new Intl.NumberFormat('ja',{style:'unit',unit:'kilometer-per-hour',unitDisplay:'long'}).formatToParts(1).map(p => p.value).join('') === '\u{6642}\u{901f}1\u{30ad}\u{30ed}\u{30e1}\u{30fc}\u{30c8}\u{30eb}'",
    ]);
}

#[test]
fn unit_validation() {
    let cases = [
        throws(
            "RangeError",
            "new Intl.NumberFormat('en',{style:'unit',unit:'foo'})",
        ),
        throws("RangeError", "new Intl.NumberFormat('en',{unit:'foo'})"),
        throws(
            "RangeError",
            "new Intl.NumberFormat('en',{style:'unit',unit:'kB'})",
        ),
        throws(
            "RangeError",
            "new Intl.NumberFormat('en',{style:'unit',unit:'meter-per-foo'})",
        ),
        throws(
            "RangeError",
            "new Intl.NumberFormat('en',{style:'unit',unit:'meter-per-second-per-hour'})",
        ),
        throws("TypeError", "new Intl.NumberFormat('en',{style:'unit'})"),
        throws(
            "RangeError",
            "new Intl.NumberFormat('en',{style:'unit',unit:'meter',unitDisplay:'wide'})",
        ),
    ];
    let refs: Vec<&str> = cases.iter().map(String::as_str).collect();
    expect_true(&refs);
    expect_true(&[
        "new Intl.NumberFormat('en',{unit:'meter'}).resolvedOptions().unit === undefined",
        "Intl.supportedValuesOf('unit').every(a => Intl.supportedValuesOf('unit').every(b => new Intl.NumberFormat('en',{style:'unit',unit:a+'-per-'+b}).format(123) !== '123'))",
    ]);
}

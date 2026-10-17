//! `Intl.Locale` (ECMA-402 14) on engine-native. Week info and time zones belong to the
//! enumeration task, so they are not covered here.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "intl_locale.js".into(),
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
        assert_eq!(
            eval(&mut iso, code),
            Value::String(JsString::from(*expected)),
            "{code}"
        );
    }
}

fn expect_true(cases: &[&str]) {
    let mut iso = isolate();
    for code in cases {
        assert_eq!(eval(&mut iso, code), Value::Bool(true), "{code}");
    }
}

/// `new Intl.Locale(...)`-style code that must throw `kind`.
fn expect_throws(kind: &str, cases: &[&str]) {
    let mut iso = isolate();
    for code in cases {
        let wrapped = format!("try {{ {code}; 'no throw' }} catch (e) {{ e.constructor.name }}");
        assert_eq!(
            eval(&mut iso, &wrapped),
            Value::String(JsString::from(kind)),
            "{code}"
        );
    }
}

#[test]
fn tag_parts() {
    expect_strings(&[
        (
            "new Intl.Locale('en-Latn-US-u-ca-gregory-hc-h12').baseName",
            "en-Latn-US",
        ),
        (
            "new Intl.Locale('en-Latn-US-u-ca-gregory-hc-h12').calendar",
            "gregory",
        ),
        (
            "new Intl.Locale('en-Latn-US-u-ca-gregory-hc-h12').hourCycle",
            "h12",
        ),
        ("new Intl.Locale('EN-latn-us').toString()", "en-Latn-US"),
        ("new Intl.Locale('pt-BR').language", "pt"),
        ("new Intl.Locale('zh-Hant-TW').script", "Hant"),
        ("new Intl.Locale('pt-BR').region", "BR"),
        ("String(new Intl.Locale('pt').region)", "undefined"),
        (
            "new Intl.Locale('en-u-co-phonebk-kf-upper-nu-latn').collation",
            "phonebk",
        ),
        ("new Intl.Locale('en-u-kf-upper').caseFirst", "upper"),
        ("new Intl.Locale('en-u-nu-latn').numberingSystem", "latn"),
        ("String(new Intl.Locale('en').calendar)", "undefined"),
        (
            "Object.prototype.toString.call(new Intl.Locale('en'))",
            "[object Intl.Locale]",
        ),
    ]);
}

#[test]
fn options_override_the_tag() {
    expect_strings(&[
        (
            "new Intl.Locale('en-US', { language: 'pt', region: 'BR' }).toString()",
            "pt-BR",
        ),
        (
            "new Intl.Locale('en', { script: 'Latn' }).toString()",
            "en-Latn",
        ),
        (
            "new Intl.Locale('en', { calendar: 'buddhist', hourCycle: 'h23' }).toString()",
            "en-u-ca-buddhist-hc-h23",
        ),
        (
            "new Intl.Locale('en', { collation: 'emoji', caseFirst: 'lower' }).toString()",
            "en-u-co-emoji-kf-lower",
        ),
        (
            "new Intl.Locale('en', { numberingSystem: 'arab' }).toString()",
            "en-u-nu-arab",
        ),
        (
            "new Intl.Locale('en-u-ca-hebrew', { calendar: 'gregory' }).calendar",
            "gregory",
        ),
        (
            "new Intl.Locale('en', { numeric: true }).toString()",
            "en-u-kn",
        ),
        (
            "new Intl.Locale('en-u-kn', { numeric: false }).toString()",
            "en-u-kn-false",
        ),
        (
            "new Intl.Locale(new Intl.Locale('pt-BR'), { region: 'PT' }).toString()",
            "pt-PT",
        ),
        (
            "new Intl.Locale('en', { firstDayOfWeek: 'mon' }).firstDayOfWeek",
            "mon",
        ),
    ]);
    expect_true(&[
        "new Intl.Locale('en', { numeric: true }).numeric === true",
        "new Intl.Locale('en').numeric === false",
        "new Intl.Locale('en-u-kn-false').numeric === false",
    ]);
}

#[test]
fn maximize_and_minimize() {
    expect_strings(&[
        ("new Intl.Locale('en').maximize().toString()", "en-Latn-US"),
        ("new Intl.Locale('en-Latn-US').minimize().toString()", "en"),
        (
            "new Intl.Locale('zh-TW').maximize().toString()",
            "zh-Hant-TW",
        ),
        (
            "new Intl.Locale('en-Latn-US-u-ca-gregory').minimize().toString()",
            "en-u-ca-gregory",
        ),
    ]);
}

#[test]
fn info_methods_and_accessors() {
    expect_strings(&[
        ("new Intl.Locale('ar').getTextInfo().direction", "rtl"),
        ("new Intl.Locale('en').getTextInfo().direction", "ltr"),
        ("new Intl.Locale('en-Hebr').textInfo.direction", "rtl"),
        ("new Intl.Locale('en-US').getCalendars()[0]", "gregory"),
        ("new Intl.Locale('th-TH').calendars[0]", "buddhist"),
        (
            "new Intl.Locale('en-u-ca-japanese').getCalendars().join()",
            "japanese",
        ),
        ("new Intl.Locale('en-US').getHourCycles().join()", "h12"),
        ("new Intl.Locale('pt-BR').hourCycles.join()", "h23"),
        (
            "new Intl.Locale('en-u-hc-h23').getHourCycles().join()",
            "h23",
        ),
        ("new Intl.Locale('en').getNumberingSystems().join()", "latn"),
        (
            "new Intl.Locale('en-u-nu-arab').numberingSystems.join()",
            "arab",
        ),
        (
            "new Intl.Locale('en').getCollations().includes('emoji') ? 'y' : 'n'",
            "y",
        ),
        (
            "new Intl.Locale('en-u-co-phonebk').collations.join()",
            "phonebk",
        ),
    ]);
}

#[test]
fn shape_and_errors() {
    expect_true(&[
        "Intl.Locale.length === 1",
        "Intl.Locale.name === 'Intl.Locale' || Intl.Locale.name === 'Locale'",
        "Intl.Locale.prototype[Symbol.toStringTag] === 'Intl.Locale'",
        "Object.getPrototypeOf(Intl.Locale) === Function.prototype",
        "typeof Object.getOwnPropertyDescriptor(Intl.Locale.prototype, 'baseName').get === 'function'",
    ]);
    expect_throws(
        "RangeError",
        &[
            "new Intl.Locale('x')",
            "new Intl.Locale('')",
            "new Intl.Locale('en', { language: '12' })",
            "new Intl.Locale('en', { region: 'abc' })",
            "new Intl.Locale('en', { hourCycle: 'h99' })",
            "new Intl.Locale('en', { caseFirst: 'x' })",
            "new Intl.Locale('en', { calendar: 'a' })",
        ],
    );
    expect_throws(
        "TypeError",
        &[
            "Intl.Locale('en')",
            "new Intl.Locale()",
            "new Intl.Locale(5)",
            "new Intl.Locale(null)",
            "Intl.Locale.prototype.toString.call({})",
        ],
    );
}

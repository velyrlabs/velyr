//! `Intl.DisplayNames`. `en`/`pt` cases run with and without `intl-experimental`.

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
            name: "intl_display.js".into(),
            text: (*code).into(),
        };
        match iso.eval_script(&source) {
            Ok(value) => assert_eq!(value, Value::Bool(true), "{code}"),
            Err(error) => panic!("`{code}` threw {}", error.message),
        }
    }
}

fn throws(kind: &str, body: &str) -> String {
    format!("(() => {{ try {{ {body}; }} catch (e) {{ return e instanceof {kind}; }} return false; }})()")
}

#[test]
fn names_in_en_and_pt() {
    expect_true(&[
        "new Intl.DisplayNames('en',{type:'region'}).of('US') === 'United States'",
        "new Intl.DisplayNames('en',{type:'language'}).of('fr') === 'French'",
        "new Intl.DisplayNames('en',{type:'currency'}).of('EUR') === 'Euro'",
        "new Intl.DisplayNames('en',{type:'script'}).of('Latn') === 'Latin'",
        "new Intl.DisplayNames('pt',{type:'region'}).of('BR') === 'Brasil'",
        "new Intl.DisplayNames('en',{type:'calendar'}).of('gregory') === 'Gregorian Calendar'",
        "new Intl.DisplayNames('en',{type:'dateTimeField'}).of('year') === 'year'",
        "new Intl.DisplayNames('pt',{type:'dateTimeField'}).of('month') === 'm\\u00eas'",
    ]);
}

#[test]
fn fallback_and_canonical_codes() {
    expect_true(&[
        "new Intl.DisplayNames('en',{type:'region'}).of('XX') === 'XX'",
        "new Intl.DisplayNames('en',{type:'region',fallback:'none'}).of('XX') === undefined",
        "new Intl.DisplayNames('en',{type:'region'}).of('us') === 'United States'",
        "new Intl.DisplayNames('en',{type:'currency'}).of('xyz') === 'XYZ'",
        "new Intl.DisplayNames('en',{type:'script'}).of('latn') === 'Latin'",
    ]);
}

#[test]
fn rejects_bad_input() {
    expect_true(&[
        &throws("RangeError", "new Intl.DisplayNames('en',{type:'language'}).of('u')"),
        &throws("RangeError", "new Intl.DisplayNames('en',{type:'region'}).of('USA')"),
        &throws("RangeError", "new Intl.DisplayNames('en',{type:'currency'}).of('EU')"),
        &throws("RangeError", "new Intl.DisplayNames('en',{type:'dateTimeField'}).of('foo')"),
        &throws("TypeError", "new Intl.DisplayNames('en')"),
        &throws("TypeError", "new Intl.DisplayNames('en',{type:'region'}).of(Symbol())"),
        &throws("TypeError", "Intl.DisplayNames('en',{type:'region'})"),
        &throws("RangeError", "new Intl.DisplayNames('en',{type:'x'})"),
    ]);
}

#[test]
fn resolved_options_and_shape() {
    expect_true(&[
        "JSON.stringify(new Intl.DisplayNames('en',{type:'region'}).resolvedOptions()) === '{\"locale\":\"en\",\"style\":\"long\",\"type\":\"region\",\"fallback\":\"code\"}'",
        "JSON.stringify(new Intl.DisplayNames('en',{type:'language'}).resolvedOptions()) === '{\"locale\":\"en\",\"style\":\"long\",\"type\":\"language\",\"fallback\":\"code\",\"languageDisplay\":\"dialect\"}'",
        "Object.prototype.toString.call(new Intl.DisplayNames('en',{type:'region'})) === '[object Intl.DisplayNames]'",
        "Intl.DisplayNames.supportedLocalesOf(['en','pt']).join() === 'en,pt'",
        "Intl.DisplayNames.length === 2",
    ]);
}

#[cfg(feature = "intl-experimental")]
#[test]
fn icu_language_display() {
    expect_true(&[
        "new Intl.DisplayNames('en',{type:'language'}).of('en-GB') === 'British English'",
        "new Intl.DisplayNames('en',{type:'language',languageDisplay:'standard'}).of('en-GB') === 'English (United Kingdom)'",
        "new Intl.DisplayNames('en',{type:'region'}).of('419') === 'Latin America'",
        "new Intl.DisplayNames('pt',{type:'language'}).of('en') === 'ingl\\u00eas'",
    ]);
}

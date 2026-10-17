//! `Intl.DurationFormat`. `en`/`pt` cases run with and without `intl-experimental`.

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
            name: "intl_duration.js".into(),
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

#[test]
fn formats_styles() {
    expect_true(&[
        "new Intl.DurationFormat('en').format({hours:1,minutes:2,seconds:3}) === '1 hr, 2 min, 3 sec'",
        "new Intl.DurationFormat('en',{style:'long'}).format({hours:1,minutes:2,seconds:3}) === '1 hour, 2 minutes, 3 seconds'",
        "new Intl.DurationFormat('en',{style:'narrow'}).format({hours:1,minutes:2,seconds:3}) === '1h 2m 3s'",
        "new Intl.DurationFormat('en',{style:'digital'}).format({hours:1,minutes:2,seconds:3}) === '1:02:03'",
        "new Intl.DurationFormat('en',{style:'digital'}).format({seconds:1,milliseconds:500}) === '0:00:01.5'",
        "new Intl.DurationFormat('en',{style:'digital'}).format({days:1,hours:1,minutes:2,seconds:3}) === '1 day, 1:02:03'",
        "new Intl.DurationFormat('en').format({years:2,days:1}) === '2 yrs, 1 day'",
        "new Intl.DurationFormat('en').format({hours:0,minutes:5}) === '5 min'",
        "new Intl.DurationFormat('en',{minutesDisplay:'always',style:'long'}).format({hours:1}) === '1 hour, 0 minutes'",
        "new Intl.DurationFormat('en',{seconds:'numeric',milliseconds:'numeric',fractionalDigits:3}).format({seconds:1,milliseconds:5}) === '1.005'",
        "new Intl.DurationFormat('en').format({hours:-1,minutes:-2}) === '-1 hr, 2 min'",
        "new Intl.DurationFormat('en').format({hours:0}) === ''",
        "new Intl.DurationFormat('pt',{style:'long'}).format({hours:1,minutes:2}) === '1 hora e 2 minutos'",
    ]);
}

#[test]
fn parts_and_resolved_options() {
    expect_true(&[
        "new Intl.DurationFormat('en').formatToParts({seconds:1}).some(p => p.type === 'unit' && p.unit === 'second')",
        "JSON.stringify(new Intl.DurationFormat('en').formatToParts({seconds:1})) === '[{\"type\":\"integer\",\"value\":\"1\",\"unit\":\"second\"},{\"type\":\"literal\",\"value\":\" \",\"unit\":\"second\"},{\"type\":\"unit\",\"value\":\"sec\",\"unit\":\"second\"}]'",
        "new Intl.DurationFormat('en').resolvedOptions().style === 'short'",
        "Object.keys(new Intl.DurationFormat('en').resolvedOptions()).slice(0,4).join() === 'locale,numberingSystem,style,years'",
        "new Intl.DurationFormat('en',{style:'digital'}).resolvedOptions().hours === 'numeric'",
        "new Intl.DurationFormat('en',{style:'digital'}).resolvedOptions().minutes === '2-digit'",
        "Object.prototype.toString.call(new Intl.DurationFormat()) === '[object Intl.DurationFormat]'",
        "Intl.DurationFormat.supportedLocalesOf(['en','pt']).join() === 'en,pt'",
        "Intl.DurationFormat.length === 0",
    ]);
}

#[test]
fn rejects_bad_input() {
    let throws = |kind: &str, body: &str| format!("({THROWS})({kind}, () => {{ {body} }})");
    expect_true(&[
        &throws(
            "RangeError",
            "new Intl.DurationFormat('en').format({hours:1,minutes:-1})",
        ),
        &throws(
            "RangeError",
            "new Intl.DurationFormat('en').format({days:1.5})",
        ),
        &throws("TypeError", "new Intl.DurationFormat('en').format(1)"),
        &throws("TypeError", "new Intl.DurationFormat('en').format({foo:1})"),
        &throws("TypeError", "Intl.DurationFormat('en')"),
        &throws("RangeError", "new Intl.DurationFormat('en', {style:'x'})"),
        &throws(
            "RangeError",
            "new Intl.DurationFormat('en', {fractionalDigits:10})",
        ),
        &throws(
            "RangeError",
            "new Intl.DurationFormat('en', {hours:'numeric', minutes:'long'})",
        ),
    ]);
}

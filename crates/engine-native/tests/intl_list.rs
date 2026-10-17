//! `Intl.ListFormat`. `en`/`pt` cases run with and without `intl-experimental`.

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
            name: "intl_list.js".into(),
            text: (*code).into(),
        };
        match iso.eval_script(&source) {
            Ok(value) => assert_eq!(value, Value::Bool(true), "{code}"),
            Err(error) => panic!("`{code}` threw {}", error.message),
        }
    }
}

#[test]
fn formats_en_and_pt() {
    expect_true(&[
        "new Intl.ListFormat('en').format(['a','b','c']) === 'a, b, and c'",
        "new Intl.ListFormat('en',{type:'disjunction'}).format(['a','b','c']) === 'a, b, or c'",
        "new Intl.ListFormat('en').format(['a','b']) === 'a and b'",
        "new Intl.ListFormat('en').format(['a']) === 'a' && new Intl.ListFormat('en').format([]) === ''",
        "new Intl.ListFormat('pt').format(['a','b']) === 'a e b'",
        "new Intl.ListFormat('pt').format(['a','b','c']) === 'a, b e c'",
        "new Intl.ListFormat('en',{style:'narrow',type:'unit'}).format(['a','b','c']) === 'a b c'",
        "new Intl.ListFormat('en').format(new Set(['x','y'])) === 'x and y'",
        "new Intl.ListFormat('en').format() === ''",
    ]);
}

#[test]
fn parts_and_resolved_options() {
    expect_true(&[
        "new Intl.ListFormat('en').formatToParts(['a','b']).map(p => p.type).join() === 'element,literal,element'",
        "JSON.stringify(new Intl.ListFormat('en').formatToParts(['a','b'])) === '[{\"type\":\"element\",\"value\":\"a\"},{\"type\":\"literal\",\"value\":\" and \"},{\"type\":\"element\",\"value\":\"b\"}]'",
        "Object.keys(new Intl.ListFormat('en').resolvedOptions()).join() === 'locale,type,style'",
        "new Intl.ListFormat('pt-BR',{type:'unit',style:'short'}).resolvedOptions().style === 'short'",
        "Object.prototype.toString.call(new Intl.ListFormat()) === '[object Intl.ListFormat]'",
        "Intl.ListFormat.supportedLocalesOf(['en','pt']).join() === 'en,pt'",
        "Intl.ListFormat.length === 0",
    ]);
}

#[test]
fn rejects_bad_input() {
    expect_true(&[
        "(() => { try { new Intl.ListFormat('en').format([1]); } catch (e) { return e instanceof TypeError; } })()",
        "(() => { try { Intl.ListFormat('en'); } catch (e) { return e instanceof TypeError; } })()",
        "(() => { try { new Intl.ListFormat('en', {type: 'x'}); } catch (e) { return e instanceof RangeError; } })()",
        "(() => { try { new Intl.ListFormat('en', 1); } catch (e) { return e instanceof TypeError; } })()",
        "(() => { let closed = false; const it = { [Symbol.iterator]() { return { next() { return { value: 1, done: false }; }, return() { closed = true; return {}; } }; } }; \
           try { new Intl.ListFormat('en').format(it); } catch (e) { return closed; } })()",
    ]);
}

#[cfg(feature = "intl-experimental")]
#[test]
fn icu_styles() {
    expect_true(&[
        "new Intl.ListFormat('en',{style:'short'}).format(['a','b','c']) === 'a, b, & c'",
        "new Intl.ListFormat('en',{style:'narrow'}).format(['a','b','c']) === 'a, b, c'",
        "new Intl.ListFormat('en',{type:'unit',style:'long'}).format(['a','b','c']) === 'a, b, c'",
    ]);
}

#[cfg(all(feature = "intl-experimental", feature = "intl-all-locales"))]
#[test]
fn icu_other_locales() {
    expect_true(&[
        "new Intl.ListFormat('de').format(['a','b','c']) === 'a, b und c'",
        "new Intl.ListFormat('es',{type:'disjunction'}).format(['a','b']) === 'a o b'",
    ]);
}

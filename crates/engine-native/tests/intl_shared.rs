//! Shared Intl helpers: CanonicalizeLocaleList, GetOption and supportedLocalesOf.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::NativeEngine;

fn expect_true(cases: &[&str]) {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    for code in cases {
        let source = Source {
            name: "intl_shared.js".into(),
            text: (*code).into(),
        };
        match iso.eval_script(&source) {
            Ok(value) => assert_eq!(value, Value::Bool(true), "{code}"),
            Err(error) => panic!("`{code}` threw {}", error.message),
        }
    }
}

fn throws(kind: &str, body: &str) -> String {
    format!(
        "(() => {{ try {{ {body}; }} catch (e) {{ return e instanceof {kind}; }} return false; }})()"
    )
}

#[test]
fn locale_list_rejects_invalid_elements() {
    expect_true(&[
        &throws("TypeError", "Intl.getCanonicalLocales(null)"),
        &throws("TypeError", "Intl.getCanonicalLocales([null])"),
        &throws("TypeError", "Intl.getCanonicalLocales([undefined])"),
        &throws("TypeError", "Intl.getCanonicalLocales([true])"),
        &throws("TypeError", "Intl.getCanonicalLocales([2])"),
        &throws("TypeError", "Intl.getCanonicalLocales([Symbol()])"),
        &throws("RangeError", "Intl.getCanonicalLocales(['en-us-'])"),
        &throws("RangeError", "Intl.getCanonicalLocales('')"),
        &throws("TypeError", "new Intl.ListFormat(null)"),
    ]);
}

#[test]
fn locale_list_reads_length_with_to_length() {
    expect_true(&[
        &throws(
            "RangeError",
            "Intl.getCanonicalLocales({ get length() { throw new RangeError(); } })",
        ),
        &throws(
            "EvalError",
            "Intl.getCanonicalLocales({ length: { valueOf() { throw new EvalError(); } } })",
        ),
        &throws(
            "TypeError",
            "Intl.getCanonicalLocales({ length: Symbol() })",
        ),
        &throws("TypeError", "Intl.getCanonicalLocales({ length: 1n })"),
        &throws(
            "EvalError",
            "new Intl.DisplayNames({ get length() { throw new EvalError(); } })",
        ),
        "Intl.getCanonicalLocales({ length: '1', 0: 'en' }).join() === 'en'",
        "Intl.getCanonicalLocales({ length: -3, 0: 'en' }).length === 0",
    ]);
}

#[test]
fn locale_list_skips_holes_in_spec_order() {
    expect_true(&[
        "Intl.getCanonicalLocales({ length: 3, 0: 'en', 2: 'pt' }).join() === 'en,pt'",
        "(() => { const log = []; const p = new Proxy({ length: 2, 0: 'en', 1: 'pt' }, {
            has(t, k) { log.push('has:' + String(k)); return k in t; },
            get(t, k) { log.push('get:' + String(k)); return t[k]; } });
          Intl.getCanonicalLocales(p);
          return log.join() === 'get:length,has:0,get:0,has:1,get:1'; })()",
        "(() => { const push = Array.prototype.push; Array.prototype.push = () => { throw 42; };
          try { return Intl.getCanonicalLocales(['en-US']).join() === 'en-US'; }
          finally { Array.prototype.push = push; } })()",
    ]);
}

#[test]
fn long_language_subtags_canonicalize() {
    expect_true(&[
        "Intl.getCanonicalLocales('zzzzzzzz-abcd-ab-abcde')[0] === 'zzzzzzzz-Abcd-AB-abcde'",
        "Intl.getCanonicalLocales('ABCDEFGH')[0] === 'abcdefgh'",
        "Intl.getCanonicalLocales('posix')[0] === 'posix'",
        "Intl.getCanonicalLocales('abcde-x-foo')[0] === 'abcde-x-foo'",
        &throws("RangeError", "Intl.getCanonicalLocales('abcdefghi')"),
    ]);
}

#[test]
fn get_option_uses_to_string() {
    expect_true(&[
        &throws("TypeError", "new Intl.ListFormat('en', { type: Symbol() })"),
        &throws("TypeError", "new Intl.DisplayNames('en', { type: 'language', fallback: Symbol() })"),
        &throws("EvalError", "new Intl.ListFormat('en', { style: { toString() { throw new EvalError(); } } })"),
        &throws("RangeError", "new Intl.ListFormat('en', { style: 'bogus' })"),
        "new Intl.ListFormat('en', { style: undefined }).resolvedOptions().style === 'long'",
        "(() => { const log = []; new Intl.ListFormat('en', new Proxy({}, { get(t, k) { log.push(k); } }));
          return log.join() === 'localeMatcher,type,style'; })()",
    ]);
}

#[test]
fn supported_locales_of_options() {
    expect_true(&[
        &throws("TypeError", "Intl.ListFormat.supportedLocalesOf([], null)"),
        &throws(
            "RangeError",
            "Intl.ListFormat.supportedLocalesOf('en', { localeMatcher: 'x' })",
        ),
        &throws(
            "EvalError",
            "Intl.ListFormat.supportedLocalesOf('en', { localeMatcher: { toString() { throw new EvalError(); } } })",
        ),
        &throws(
            "TypeError",
            "Intl.ListFormat.supportedLocalesOf('en', { localeMatcher: Symbol() })",
        ),
        "Intl.ListFormat.supportedLocalesOf(['en', 'pt'], { localeMatcher: 'lookup' }).join() === 'en,pt'",
        "Intl.ListFormat.supportedLocalesOf(['en', 'zxx']).join() === 'en'",
    ]);
}

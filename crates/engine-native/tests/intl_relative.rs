//! `Intl.RelativeTimeFormat`. `en`/`pt` cases run with and without `intl-experimental`.

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
            name: "intl_relative.js".into(),
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
        "new Intl.RelativeTimeFormat('en').format(-1,'day') === '1 day ago'",
        "new Intl.RelativeTimeFormat('en').format(2,'hours') === 'in 2 hours'",
        "new Intl.RelativeTimeFormat('en').format(-0,'second') === '0 seconds ago'",
        "new Intl.RelativeTimeFormat('en').format(0,'second') === 'in 0 seconds'",
        "new Intl.RelativeTimeFormat('en').format(1.5,'day') === 'in 1.5 days'",
        "new Intl.RelativeTimeFormat('en').format(1234,'year') === 'in 1,234 years'",
        "new Intl.RelativeTimeFormat('en').format(3,'quarter') === 'in 3 quarters'",
        "new Intl.RelativeTimeFormat('pt').format(1,'day') === 'em 1 dia'",
        "new Intl.RelativeTimeFormat('pt').format(-2,'day') === 'há 2 dias'",
    ]);
}

#[test]
fn numeric_auto() {
    expect_true(&[
        "new Intl.RelativeTimeFormat('en',{numeric:'auto'}).format(-1,'day') === 'yesterday'",
        "new Intl.RelativeTimeFormat('en',{numeric:'auto'}).format(0,'day') === 'today'",
        "new Intl.RelativeTimeFormat('en',{numeric:'auto'}).format(-0,'day') === 'today'",
        "new Intl.RelativeTimeFormat('en',{numeric:'auto'}).format(1,'day') === 'tomorrow'",
        "new Intl.RelativeTimeFormat('en',{numeric:'auto'}).format(1,'year') === 'next year'",
        "new Intl.RelativeTimeFormat('en',{numeric:'auto'}).format(2,'day') === 'in 2 days'",
        "new Intl.RelativeTimeFormat('en',{numeric:'auto'}).format(1.0004,'day') === 'in 1 day'",
        "new Intl.RelativeTimeFormat('pt',{numeric:'auto'}).format(-1,'day') === 'ontem'",
    ]);
}

#[test]
fn styles() {
    expect_true(&[
        "new Intl.RelativeTimeFormat('en',{style:'short'}).format(-3,'month') === '3 mo. ago'",
        "new Intl.RelativeTimeFormat('en',{style:'short'}).format(2,'hour') === 'in 2 hr.'",
    ]);
}

#[test]
fn parts_and_resolved_options() {
    expect_true(&[
        "JSON.stringify(new Intl.RelativeTimeFormat('en').formatToParts(3,'day')) === '[{\"type\":\"literal\",\"value\":\"in \"},{\"type\":\"integer\",\"value\":\"3\",\"unit\":\"day\"},{\"type\":\"literal\",\"value\":\" days\"}]'",
        "JSON.stringify(new Intl.RelativeTimeFormat('en',{numeric:'auto'}).formatToParts(-1,'day')) === '[{\"type\":\"literal\",\"value\":\"yesterday\"}]'",
        "new Intl.RelativeTimeFormat('en').formatToParts(-1234.5,'second').map(p => p.type).join() === 'integer,group,integer,decimal,fraction,literal'",
        "Object.keys(new Intl.RelativeTimeFormat('en').resolvedOptions()).join() === 'locale,style,numeric,numberingSystem'",
        "new Intl.RelativeTimeFormat('en').resolvedOptions().numberingSystem === 'latn'",
        "Object.prototype.toString.call(new Intl.RelativeTimeFormat()) === '[object Intl.RelativeTimeFormat]'",
        "Intl.RelativeTimeFormat.supportedLocalesOf(['en','pt']).join() === 'en,pt'",
        "Intl.RelativeTimeFormat.length === 0",
    ]);
}

#[test]
fn rejects_bad_input() {
    expect_true(&[
        "(() => { try { new Intl.RelativeTimeFormat('en').format(1,'fortnight'); } catch (e) { return e instanceof RangeError; } })()",
        "(() => { try { new Intl.RelativeTimeFormat('en').format(Infinity,'day'); } catch (e) { return e instanceof RangeError; } })()",
        "(() => { try { new Intl.RelativeTimeFormat('en').format(NaN,'day'); } catch (e) { return e instanceof RangeError; } })()",
        "(() => { try { Intl.RelativeTimeFormat('en'); } catch (e) { return e instanceof TypeError; } })()",
        "(() => { try { new Intl.RelativeTimeFormat('en', {style: 'x'}); } catch (e) { return e instanceof RangeError; } })()",
        "(() => { try { new Intl.RelativeTimeFormat('en', null); } catch (e) { return e instanceof TypeError; } })()",
        "(() => { try { Intl.RelativeTimeFormat.prototype.format.call({}, 1, 'day'); } catch (e) { return e instanceof TypeError; } })()",
    ]);
}

#[cfg(feature = "intl-all-locales")]
#[test]
fn icu_other_locales() {
    expect_true(&[
        "new Intl.RelativeTimeFormat('de').format(-1,'day') === 'vor 1 Tag'",
        "new Intl.RelativeTimeFormat('es',{numeric:'auto'}).format(1,'day') === 'mañana'",
    ]);
}

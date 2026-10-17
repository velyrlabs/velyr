//! `Intl.supportedValuesOf` and the week info and time zone members of `Intl.Locale`.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    let source = Source {
        name: "intl_enumeration.js".into(),
        text: code.into(),
    };
    match iso.eval_script(&source) {
        Ok(value) => value,
        Err(error) => panic!("`{code}` threw {}", error.message),
    }
}

fn expect_true(cases: &[&str]) {
    let mut iso = isolate();
    for code in cases {
        assert_eq!(eval(&mut iso, code), Value::Bool(true), "{code}");
    }
}

#[test]
fn values_are_sorted_and_unique() {
    expect_true(&[
        "Intl.supportedValuesOf('calendar').includes('gregory')",
        "['calendar','collation','currency','numberingSystem','timeZone','unit'].every(k => { \
           const a = Intl.supportedValuesOf(k); return a.join() === [...new Set(a)].sort().join(); })",
        "Intl.supportedValuesOf('timeZone').includes('America/Sao_Paulo')",
        "Intl.supportedValuesOf('unit').includes('kilometer-per-hour') === false \
           && Intl.supportedValuesOf('unit').includes('kilometer')",
        "Intl.supportedValuesOf('currency').includes('BRL')",
        "Intl.supportedValuesOf('numberingSystem').includes('latn')",
        "Intl.supportedValuesOf.length === 1 && Intl.supportedValuesOf.name === 'supportedValuesOf'",
    ]);
}

#[test]
fn unknown_key_throws() {
    expect_true(&[
        "(() => { try { Intl.supportedValuesOf('foo'); } catch (e) { return e instanceof RangeError; } })()",
    ]);
}

#[test]
fn values_are_accepted_by_constructors() {
    expect_true(&[
        "Intl.supportedValuesOf('timeZone').every(z => new Intl.DateTimeFormat('en', {timeZone: z}).resolvedOptions().timeZone === z)",
        "Intl.supportedValuesOf('currency').every(c => !!new Intl.NumberFormat('en', {style: 'currency', currency: c}))",
        "Intl.supportedValuesOf('calendar').every(c => new Intl.DateTimeFormat('en', {calendar: c}).resolvedOptions().calendar === c)",
        "Intl.supportedValuesOf('numberingSystem').every(n => new Intl.NumberFormat('en', {numberingSystem: n}).resolvedOptions().numberingSystem === n)",
    ]);
}

#[test]
fn locale_week_info() {
    expect_true(&[
        "new Intl.Locale('en-US').getWeekInfo().firstDay === 7",
        "new Intl.Locale('de').getWeekInfo().firstDay === 1",
        "new Intl.Locale('en-US-u-fw-wed').weekInfo.firstDay === 3",
        "new Intl.Locale('en-US-u-rg-dezzzz').getWeekInfo().firstDay === 1",
        "new Intl.Locale('en-US').getWeekInfo().weekend.join() === '6,7'",
        "new Intl.Locale('en').weekInfo.firstDay === 7",
        "Reflect.ownKeys(new Intl.Locale('en').getWeekInfo()).join() === 'firstDay,weekend'",
    ]);
}

#[test]
fn locale_time_zones() {
    expect_true(&[
        "new Intl.Locale('en').getTimeZones() === undefined",
        "new Intl.Locale('en').timeZones === undefined",
        "new Intl.Locale('en-NZ').getTimeZones().includes('Pacific/Auckland')",
        "new Intl.Locale('pt-BR').timeZones.includes('America/Sao_Paulo')",
        "(a => a.join() === [...a].sort().join())(new Intl.Locale('en-US').getTimeZones())",
    ]);
}

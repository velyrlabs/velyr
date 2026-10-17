//! `Intl.DateTimeFormat` gaps: formatRange, timeZoneName variants, option precedence.

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
            name: "intl_datetime_gaps.js".into(),
            text: (*code).into(),
        };
        match iso.eval_script(&source) {
            Ok(value) => assert_eq!(value, Value::Bool(true), "{code}"),
            Err(error) => panic!("`{code}` threw {}", error.message),
        }
    }
}

#[test]
fn style_and_hour_cycle_precedence() {
    expect_true(&[
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',dateStyle:'short'}).format(Date.UTC(2024,0,5)) === '1/5/24'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',hour:'numeric',minute:'numeric',hour12:false}).format(0) === '00:00'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',hour:'numeric',hourCycle:'h23',hour12:true}).format(0) === '12\u{202f}AM'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',hour:'numeric',hour12:true,hourCycle:'h23'}).resolvedOptions().hourCycle === 'h12'",
    ]);
}

#[test]
fn fractional_seconds() {
    expect_true(&[
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',fractionalSecondDigits:3,second:'numeric'}).format(1234) === '1.234'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',fractionalSecondDigits:2,second:'numeric'}).format(1234) === '1.23'",
    ]);
}

#[test]
fn time_zone_name_variants() {
    expect_true(&[
        "new Intl.DateTimeFormat('en',{timeZoneName:'longOffset',timeZone:'America/New_York'}).format(0).endsWith('GMT-05:00')",
        "new Intl.DateTimeFormat('en',{timeZoneName:'shortOffset',timeZone:'America/New_York'}).format(0).endsWith('GMT-5')",
        "new Intl.DateTimeFormat('en',{timeZoneName:'short',timeZone:'America/New_York'}).format(0).endsWith('EST')",
        "new Intl.DateTimeFormat('en',{timeZoneName:'long',timeZone:'America/New_York'}).format(0).endsWith('Eastern Standard Time')",
        "new Intl.DateTimeFormat('en',{timeZoneName:'longGeneric',timeZone:'America/New_York'}).format(0).endsWith('Eastern Time')",
    ]);
}

#[test]
fn format_range() {
    expect_true(&[
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',dateStyle:'short'}).formatRange(new Date(0), new Date(86400000)) === '1/1/70\u{2009}\u{2013}\u{2009}1/2/70'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',dateStyle:'short'}).formatRange(new Date(0), new Date(1000)) === '1/1/70'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',dateStyle:'short'}).formatRangeToParts(new Date(0), new Date(86400000)).map((p) => p.source).join() === 'startRange,startRange,startRange,startRange,startRange,shared,endRange,endRange,endRange,endRange,endRange'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',dateStyle:'short'}).formatRangeToParts(new Date(0), new Date(1000)).every((p) => p.source === 'shared')",
        "(() => { try { new Intl.DateTimeFormat('en').formatRange(undefined, 0); } catch (e) { return e instanceof TypeError; } return false; })()",
        "(() => { try { new Intl.DateTimeFormat('en').formatRange(0, NaN); } catch (e) { return e instanceof RangeError; } return false; })()",
    ]);
}

#[test]
fn era_and_calendar() {
    expect_true(&[
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',era:'short',year:'numeric'}).format(0).includes('AD')",
        "new Intl.DateTimeFormat('en',{calendar:'ISLAMIC-CIVIL'}).resolvedOptions().calendar === 'islamic-civil'",
        "new Intl.DateTimeFormat('en',{calendar:'gregory'}).resolvedOptions().calendar === 'gregory'",
        "new Intl.DateTimeFormat('en',{calendar:'islamicc'}).resolvedOptions().calendar === 'islamic-civil'",
        "new Intl.DateTimeFormat('en',{calendar:'ethiopic-amete-alem'}).resolvedOptions().calendar === 'ethioaa'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',year:'numeric',era:'short'}).format(-8.64e15).includes('BC')",
        "(() => { try { new Intl.DateTimeFormat('en',{calendar:'a'}); } catch (e) { return e instanceof RangeError; } return false; })()",
        "!('pattern' in new Intl.DateTimeFormat('en').resolvedOptions())",
    ]);
}

#[test]
fn locale_string_defaults() {
    expect_true(&[
        "new Date(0).toLocaleDateString('en',{timeZone:'UTC'}) === '1/1/1970'",
        "new Date(0).toLocaleTimeString('en',{timeZone:'UTC'}) === '12:00:00\u{202f}AM'",
        "new Date(0).toLocaleString('en',{timeZone:'UTC'}) === '1/1/1970, 12:00:00\u{202f}AM'",
        "new Date(0).toLocaleDateString('en',{timeZone:'UTC',hour:'numeric'}).includes('1')",
    ]);
}

#[test]
fn legacy_constructor_and_date_time_halves() {
    expect_true(&[
        "(() => { const o = new Intl.DateTimeFormat('en'); const r = Intl.DateTimeFormat.call(o); return r === o && Object.getOwnPropertySymbols(o).some((s) => s.description === 'IntlLegacyConstructedSymbol') && o.resolvedOptions().locale === 'en'; })()",
    ]);
    expect_true(&[
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',year:'numeric',hour:'numeric',minute:'numeric'}).format(0) === '1970, 12:00\u{202f}AM'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',month:'short',day:'numeric',minute:'numeric'}).format(60000) === 'Jan 1, 1'",
    ]);
}

#[test]
fn time_fields_without_hour() {
    expect_true(&[
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',minute:'numeric',second:'numeric',fractionalSecondDigits:2}).format(63456) === '01:03.45'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',second:'numeric'}).format(6000) === '6'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',minute:'numeric'}).format(360000) === '6'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',fractionalSecondDigits:3,second:'numeric'}).formatToParts(1234).map((p) => p.type).join() === 'second,literal,fractionalSecond'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',dayPeriod:'long'}).format(0) === 'at night'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',dayPeriod:'short'}).format(12 * 3600000) === 'noon'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',dayPeriod:'long'}).format(9 * 3600000) === 'in the morning'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',hour:'numeric',dayPeriod:'long'}).format(15 * 3600000) === '3 in the afternoon'",
    ]);
}

#[test]
fn default_hour_cycle_and_range_collapse() {
    expect_true(&[
        "new Intl.DateTimeFormat('en',{hour:'numeric'}).resolvedOptions().hourCycle === 'h12'",
        "new Intl.DateTimeFormat('en',{hour:'numeric',hour12:false}).resolvedOptions().hourCycle === 'h23'",
        "new Intl.DateTimeFormat('en',{minute:'numeric'}).resolvedOptions().hourCycle === undefined",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',dateStyle:'short'}).format(new Date('1886-05-01T14:12:47Z')) === '5/1/86'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',year:'numeric',month:'short',day:'numeric'}).formatRange(Date.UTC(2019,0,3), Date.UTC(2019,0,5)) === 'Jan 3\u{2009}\u{2013}\u{2009}5, 2019'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',year:'numeric',month:'short',day:'numeric'}).formatRange(Date.UTC(2019,0,3), Date.UTC(2019,2,4)) === 'Jan 3\u{2009}\u{2013}\u{2009}Mar 4, 2019'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',year:'numeric',month:'short',day:'numeric'}).formatRange(Date.UTC(2019,0,3), Date.UTC(2020,2,4)) === 'Jan 3, 2019\u{2009}\u{2013}\u{2009}Mar 4, 2020'",
        "new Intl.DateTimeFormat('en',{timeZone:'UTC',dateStyle:'short',timeStyle:'short'}).formatRange(0, 3600000).startsWith('1/1/70, 12:00')",
    ]);
}

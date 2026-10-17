//! Temporal.Duration fidelity (N25): parsing, exact sub-second fields, relativeTo and options.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::NativeEngine;

fn eval(code: &str) -> Value {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "temporal_duration_fidelity.js".into(),
        text: code.into(),
    };
    match iso.eval_script(&source) {
        Ok(value) => value,
        Err(error) => panic!("`{code}` threw {}", error.message),
    }
}

fn is(code: &str, expected: &str) {
    let expected = Value::String(JsString::from(expected));
    assert_eq!(eval(code), expected, "{code}");
}

/// `code` must throw an instance of the error constructor `kind`.
fn throws(kind: &str, code: &str) {
    let probe = format!(
        "(() => {{ try {{ {code}; return 'no throw'; }} catch (e) {{ return e instanceof {kind} ? 'ok' : String(e); }} }})()"
    );
    is(&probe, "ok");
}

#[test]
fn fractional_hours_become_minutes() {
    is("String(Temporal.Duration.from('PT0.5H').minutes)", "30");
}

#[test]
fn calendar_units_need_relative_to() {
    throws(
        "RangeError",
        "Temporal.Duration.from('P1Y').total({unit:'days'})",
    );
}

#[test]
fn total_uses_relative_to_month_length() {
    is(
        "String(Temporal.Duration.from('P1M').total({unit:'days',relativeTo:'2024-02-01'}))",
        "29",
    );
}

#[test]
fn compare_uses_relative_to() {
    is(
        "String(Temporal.Duration.compare('P1M','P30D',{relativeTo:'2024-04-01'}))",
        "0",
    );
}

#[test]
fn mixed_signs_are_rejected() {
    throws("RangeError", "Temporal.Duration.from({hours:1,minutes:-1})");
}

#[test]
fn sub_second_fields_stay_exact_above_999() {
    is(
        "const d = new Temporal.Duration(0,0,0,0,0,0,0,1500,2500,3500); [d.milliseconds,d.microseconds,d.nanoseconds,d.seconds].join()",
        "1500,2500,3500,0",
    );
    is(
        "const d = Temporal.Duration.from({milliseconds:1500}).add({microseconds:2500}); [d.milliseconds,d.microseconds].join()",
        "1502,500",
    );
}

#[test]
fn until_keeps_large_sub_second_largest_unit() {
    is(
        "const a = Temporal.Instant.fromEpochMilliseconds(0), b = Temporal.Instant.fromEpochMilliseconds(5500); const d = a.until(b, {largestUnit:'milliseconds'}); [d.seconds,d.milliseconds].join()",
        "0,5500",
    );
}

#[test]
fn non_integers_are_rejected() {
    throws("RangeError", "new Temporal.Duration(1.5)");
    throws("RangeError", "Temporal.Duration.from({years:1.5})");
    throws(
        "RangeError",
        "Temporal.Duration.from({days:1}).with({hours:0.5})",
    );
    throws("TypeError", "Temporal.Duration.from({years:Symbol()})");
}

#[test]
fn from_reads_fields_alphabetically() {
    is(
        "const log=[]; Temporal.Duration.from(new Proxy({days:1},{get(t,k){log.push(k);return t[k]}})); log.join()",
        "days,hours,microseconds,milliseconds,minutes,months,nanoseconds,seconds,weeks,years",
    );
}

#[test]
fn constructor_and_compare_lengths() {
    is(
        "[Temporal.Duration.length, Temporal.Duration.compare.length, Temporal.Duration.prototype.toString.length].join()",
        "0,2,0",
    );
}

#[test]
fn to_string_validates_fractional_second_digits() {
    throws(
        "RangeError",
        "new Temporal.Duration(1).toString({fractionalSecondDigits:null})",
    );
    throws(
        "RangeError",
        "new Temporal.Duration(1).toString({fractionalSecondDigits:-0.6})",
    );
    is(
        "new Temporal.Duration(0,0,0,0,0,0,1,500).toString({fractionalSecondDigits:2})",
        "PT1.50S",
    );
}

#[test]
fn round_reads_options_in_spec_order() {
    is(
        "const log=[]; const o = new Proxy({smallestUnit:'second'},{get(t,k){log.push(k);return t[k]}}); new Temporal.Duration(0,0,0,0,1).round(o); log.join()",
        "largestUnit,relativeTo,roundingIncrement,roundingMode,smallestUnit",
    );
}

#[test]
fn total_reads_relative_to_before_unit() {
    is(
        "const log=[]; const o = new Proxy({unit:'second'},{get(t,k){log.push(k);return t[k]}}); new Temporal.Duration(0,0,0,0,1).total(o); log.join()",
        "relativeTo,unit",
    );
}

#[test]
fn string_shorthand_ignores_object_prototype() {
    is(
        "Object.prototype.relativeTo = '2020-01-01'; try { return_ = String(new Temporal.Duration(0,0,0,0,1).total('hours')); } finally { delete Object.prototype.relativeTo; } return_",
        "1",
    );
}

#[test]
fn rounding_increment_is_coerced_with_to_number() {
    is(
        "String(new Temporal.Duration(0,0,0,0,0,0,56).round({smallestUnit:'second',roundingIncrement:'5'}).seconds)",
        "55",
    );
}

#[test]
fn large_values_negate_exactly() {
    is(
        "const d = new Temporal.Duration(0,0,0,0,0,0,0,0,0,9007199254740991).negated(); String(d.nanoseconds)",
        "-9007199254740991",
    );
}

#[test]
fn relative_to_bag_reads_fields_alphabetically() {
    is(
        "const log=[]; const r = new Proxy({year:2020,month:1,day:1},{get(t,k){log.push(k);return t[k]}}); new Temporal.Duration(0,1).total({unit:'days',relativeTo:r}); log.join()",
        "calendar,day,hour,microsecond,millisecond,minute,month,monthCode,nanosecond,offset,second,timeZone,year",
    );
    throws(
        "RangeError",
        "new Temporal.Duration(0,1).total({unit:'days',relativeTo:{year:2020,month:1,day:1,hour:Infinity}})",
    );
}

#[test]
fn brand_checks_on_serializers() {
    throws("TypeError", "Temporal.Duration.prototype.toJSON.call({})");
    throws(
        "TypeError",
        "Temporal.Duration.prototype.toLocaleString.call({})",
    );
}

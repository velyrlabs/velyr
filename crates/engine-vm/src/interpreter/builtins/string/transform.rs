//! `String.prototype` methods that transform text: case mapping, `trim*`, `normalize`
//! and `localeCompare`.

use std::cmp::Ordering;

use unicode_normalization::UnicodeNormalization;

use super::super::{Interpreter, JsResult, arg};
use super::{string_value, this_units};
use crate::{ObjectRef, Value, is_js_whitespace};

pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    vm.method(proto, "localeCompare", 1, string_locale_compare)?;
    vm.method(proto, "normalize", 0, string_normalize)?;
    vm.method(proto, "toLocaleLowerCase", 0, string_to_locale_lower_case)?;
    vm.method(proto, "toLocaleUpperCase", 0, string_to_locale_upper_case)?;
    vm.method(proto, "toLowerCase", 0, string_to_lower_case)?;
    vm.method(proto, "toUpperCase", 0, string_to_upper_case)?;
    vm.method(proto, "trim", 0, string_trim)?;
    vm.method(proto, "trimEnd", 0, string_trim_end)?;
    vm.method(proto, "trimStart", 0, string_trim_start)?;
    // Annex B: trimLeft/trimRight are the same function objects as trimStart/trimEnd.
    let trim_start = vm.get_named(proto, "trimStart")?;
    vm.define_method(proto, "trimLeft", trim_start)?;
    let trim_end = vm.get_named(proto, "trimEnd")?;
    vm.define_method(proto, "trimRight", trim_end)
}

/// Applies `f` to every run of valid code points; lone surrogates pass through.
fn map_runs(units: &[u16], f: impl Fn(&str) -> String) -> Vec<u16> {
    let mut out = Vec::with_capacity(units.len());
    let mut run = String::new();
    for c in char::decode_utf16(units.iter().copied()) {
        match c {
            Ok(c) => run.push(c),
            Err(lone) => {
                out.extend(f(&run).encode_utf16());
                run.clear();
                out.push(lone.unpaired_surrogate());
            }
        }
    }
    out.extend(f(&run).encode_utf16());
    out
}

fn lower(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<Value> {
    let units = this_units(vm, this, method)?;
    Ok(string_value(map_runs(&units, str::to_lowercase)))
}

fn upper(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<Value> {
    let units = this_units(vm, this, method)?;
    Ok(string_value(map_runs(&units, str::to_uppercase)))
}

fn string_to_lower_case(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    lower(vm, this, "toLowerCase")
}

fn string_to_upper_case(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    upper(vm, this, "toUpperCase")
}

/// shortcut: locale-insensitive (root locale) mapping; tailor once Intl lands.
fn string_to_locale_lower_case(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    lower(vm, this, "toLocaleLowerCase")
}

fn string_to_locale_upper_case(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    upper(vm, this, "toLocaleUpperCase")
}

fn is_whitespace_unit(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(is_js_whitespace)
}

fn trim(
    vm: &mut Interpreter,
    this: &Value,
    method: &str,
    start: bool,
    end: bool,
) -> JsResult<Value> {
    let units = this_units(vm, this, method)?;
    let mut from = 0;
    let mut to = units.len();
    if start {
        while from < to && is_whitespace_unit(units[from]) {
            from += 1;
        }
    }
    if end {
        while to > from && is_whitespace_unit(units[to - 1]) {
            to -= 1;
        }
    }
    Ok(string_value(units[from..to].to_vec()))
}

fn string_trim(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    trim(vm, this, "trim", true, true)
}

fn string_trim_start(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    trim(vm, this, "trimStart", true, false)
}

fn string_trim_end(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    trim(vm, this, "trimEnd", false, true)
}

fn string_normalize(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "normalize")?;
    let form = match arg(args, 0) {
        Value::Undefined => "NFC".to_string(),
        value => vm.coerce_rust_string(&value)?,
    };
    let normalized = match form.as_str() {
        "NFC" => map_runs(&units, |s| s.nfc().collect()),
        "NFD" => map_runs(&units, |s| s.nfd().collect()),
        "NFKC" => map_runs(&units, |s| s.nfkc().collect()),
        "NFKD" => map_runs(&units, |s| s.nfkd().collect()),
        _ => {
            return vm.throw_range("The normalization form should be one of NFC, NFD, NFKC, NFKD.");
        }
    };
    Ok(string_value(normalized))
}

/// shortcut: canonical-equivalence-aware code point order (NFC then compare), not a
/// locale collation; replace with ICU collation once Intl lands.
fn string_locale_compare(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "localeCompare")?;
    let that = vm.coerce_string(&arg(args, 0))?.to_utf16();
    let nfc = |u: &[u16]| map_runs(u, |s| s.nfc().collect());
    let left = String::from_utf16_lossy(&nfc(&units));
    let right = String::from_utf16_lossy(&nfc(&that));
    let order = match left.cmp(&right) {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    };
    Ok(Value::Number(order))
}

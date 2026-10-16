//! ECMAScript type conversions (ECMA-262 7.1): ToPrimitive, ToBoolean, ToNumber, ToString, ToPropertyKey.
//!
//! Objects have no model yet, so conversions that reach one go through an `ObjectHook`
//! supplied by the caller (it stands in for `[Symbol.toPrimitive]` / `valueOf` / `toString`).

use velyr_engine_gc::GcRef;

use crate::value::{OpResult, SymbolId, TypeError, Value};
use crate::{Atom, Interner, JsStr, bigint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Default,
    Number,
    String,
}

pub type ObjectHook<'a> = &'a mut dyn FnMut(GcRef, Hint) -> OpResult<Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropertyKey {
    Atom(Atom),
    Symbol(SymbolId),
}

fn type_error<T>(message: &str) -> OpResult<T> {
    Err(TypeError(message.into()))
}

pub fn to_primitive(value: &Value, hint: Hint, hook: ObjectHook) -> OpResult<Value> {
    let Value::Object(object) = value else {
        return Ok(value.clone());
    };
    match hook(*object, hint)? {
        Value::Object(_) => type_error("Cannot convert object to primitive value"),
        primitive => Ok(primitive),
    }
}

pub fn to_boolean(value: &Value) -> bool {
    match value {
        Value::Undefined | Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => !(n.is_nan() || *n == 0.0),
        Value::String(s) => !s.is_empty(),
        Value::BigInt(b) => !b.magnitude_le().is_empty(),
        Value::Symbol(_) | Value::Object(_) => true,
    }
}

pub fn to_number(value: &Value, hook: ObjectHook) -> OpResult<f64> {
    match to_primitive(value, Hint::Number, hook)? {
        Value::Undefined => Ok(f64::NAN),
        Value::Null => Ok(0.0),
        Value::Bool(b) => Ok(f64::from(u8::from(b))),
        Value::Number(n) => Ok(n),
        Value::String(s) => Ok(string_to_number(&s)),
        Value::Symbol(_) => type_error("Cannot convert a Symbol value to a number"),
        Value::BigInt(_) => type_error("Cannot convert a BigInt value to a number"),
        Value::Object(_) => unreachable!("to_primitive never returns an object"),
    }
}

pub fn to_string(value: &Value, hook: ObjectHook) -> OpResult<JsStr> {
    match to_primitive(value, Hint::String, hook)? {
        Value::String(s) => Ok(s),
        Value::Undefined => Ok(JsStr::from_str("undefined")),
        Value::Null => Ok(JsStr::from_str("null")),
        Value::Bool(b) => Ok(JsStr::from_str(if b { "true" } else { "false" })),
        Value::Number(n) => Ok(JsStr::from_str(&number_to_string(n))),
        Value::BigInt(b) => Ok(JsStr::from_str(&bigint::to_decimal_string(&b))),
        Value::Symbol(_) => type_error("Cannot convert a Symbol value to a string"),
        Value::Object(_) => unreachable!("to_primitive never returns an object"),
    }
}

pub fn to_property_key(
    value: &Value,
    interner: &mut Interner,
    hook: ObjectHook,
) -> OpResult<PropertyKey> {
    match to_primitive(value, Hint::String, hook)? {
        Value::Symbol(symbol) => Ok(PropertyKey::Symbol(symbol)),
        key => Ok(PropertyKey::Atom(interner.intern(&to_string(&key, hook)?))),
    }
}

/// Number::toString(10): shortest round-trip digits (`ryu-js` implements the spec layout).
pub fn number_to_string(n: f64) -> String {
    ryu_js::Buffer::new().format(n).to_owned()
}

/// StrWhiteSpaceChar: WhiteSpace plus LineTerminator.
pub(crate) fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\u{b}' | '\u{c}' | ' ' | '\u{a0}' | '\u{feff}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\n'
                | '\r'
                | '\u{2028}'
                | '\u{2029}'
    )
}

/// StringToNumber (7.1.4.1.1): decimal literals, `Infinity`, and unsigned `0x`/`0o`/`0b` integers.
pub fn string_to_number(s: &JsStr) -> f64 {
    let text = s.to_string_lossy();
    let text = text.trim_matches(is_js_whitespace);
    match text {
        "" => return 0.0,
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let radix = match text.get(..2) {
        Some("0x" | "0X") => Some(16),
        Some("0o" | "0O") => Some(8),
        Some("0b" | "0B") => Some(2),
        _ => None,
    };
    match radix {
        Some(radix) => parse_radix(&text[2..], radix),
        None if is_decimal_literal(text) => text.parse().unwrap_or(f64::NAN),
        None => f64::NAN,
    }
}

fn parse_radix(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let mut exact = Some(0u128);
    // shortcut: past 128 bits the rounding is not exactly round-to-nearest, upgrade with a bignum parser.
    let mut approx = 0.0f64;
    for c in digits.chars() {
        let Some(digit) = c.to_digit(radix) else {
            return f64::NAN;
        };
        approx = approx * f64::from(radix) + f64::from(digit);
        exact = exact.and_then(|v| {
            v.checked_mul(u128::from(radix))?
                .checked_add(u128::from(digit))
        });
    }
    exact.map_or(approx, |v| v as f64)
}

/// StrDecimalLiteral without `Infinity`: `[+-]? (digits [. digits?]? | . digits) ([eE] [+-]? digits)?`.
fn is_decimal_literal(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut i = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let int_digits = digits(i);
    i += int_digits;
    let mut frac_digits = 0;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        frac_digits = digits(i);
        i += frac_digits;
    }
    if int_digits + frac_digits == 0 {
        return false;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1 + usize::from(matches!(bytes.get(i + 1), Some(b'+' | b'-')));
        let exp_digits = digits(i);
        if exp_digits == 0 {
            return false;
        }
        i += exp_digits;
    }
    i == bytes.len()
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use velyr_engine_api::JsBigInt;
    use velyr_engine_gc::GcRef;

    use super::*;
    use crate::{Interner, JsStr, SymbolId, TypeError, Value};

    fn s(text: &str) -> JsStr {
        JsStr::from_str(text)
    }

    fn num(text: &str) -> f64 {
        string_to_number(&s(text))
    }

    fn no_objects(_: GcRef, _: Hint) -> Result<Value, TypeError> {
        panic!("hook must not be called for primitives")
    }

    fn big(n: i64) -> Value {
        Value::BigInt(Rc::new(JsBigInt::from_i64(n)))
    }

    #[test]
    fn string_to_number_trims_whitespace_and_line_terminators() {
        assert_eq!(num("  12  "), 12.0);
        assert_eq!(num("\t\u{b}\u{c} 7 \u{a0}\u{feff}\r\n"), 7.0);
        assert_eq!(
            num("\u{2028}\u{2029}\u{1680}\u{2000}\u{200a}\u{202f}\u{205f}\u{3000}8"),
            8.0
        );
        assert!(num("\u{180e}1").is_nan(), "U+180E is not whitespace");
        assert!(num("\u{200b}1").is_nan(), "U+200B is not whitespace");
    }

    #[test]
    fn string_to_number_empty_and_blank_are_zero() {
        assert_eq!(num(""), 0.0);
        assert_eq!(num("   \n\t"), 0.0);
    }

    #[test]
    fn string_to_number_radix_prefixes() {
        assert_eq!(num("0x1F"), 31.0);
        assert_eq!(num("0X1f"), 31.0);
        assert_eq!(num("0b101"), 5.0);
        assert_eq!(num("0B11"), 3.0);
        assert_eq!(num("0o17"), 15.0);
        assert_eq!(num("0O7"), 7.0);
        assert_eq!(num("0xFFFFFFFFFFFFFFFFF"), 295147905179352825856.0);
        for bad in [
            "0x", "-0x10", "+0x10", "0b102", "0o8", "0xG", "0x1_0", "0x1.8", "0b",
        ] {
            assert!(num(bad).is_nan(), "{bad:?}");
        }
    }

    #[test]
    fn string_to_number_decimal_grammar() {
        assert_eq!(num("1e3"), 1000.0);
        assert_eq!(num(".5"), 0.5);
        assert_eq!(num("5."), 5.0);
        assert_eq!(num("+1.5E-1"), 0.15);
        assert_eq!(num("1e1000"), f64::INFINITY);
        assert_eq!(num("007"), 7.0);
        for bad in [
            "1_000", "1e", "e1", ".", "+", "-", "+-1", "1 2", "12px", "1,5", "0x", "\u{661}",
        ] {
            assert!(num(bad).is_nan(), "{bad:?}");
        }
    }

    #[test]
    fn string_to_number_signed_zero_and_infinity() {
        assert!(num("-0").is_sign_negative() && num("-0") == 0.0);
        assert!(num("+0").is_sign_positive());
        assert_eq!(num("Infinity"), f64::INFINITY);
        assert_eq!(num("+Infinity"), f64::INFINITY);
        assert_eq!(num(" -Infinity "), f64::NEG_INFINITY);
        for bad in [
            "infinity", "inf", "INFINITY", "Infinit", "NaN", "nan", "-NaN",
        ] {
            assert!(num(bad).is_nan(), "{bad:?}");
        }
    }

    #[test]
    fn number_to_string_special_values() {
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(0.0), "0");
        assert_eq!(number_to_string(f64::NAN), "NaN");
        assert_eq!(number_to_string(f64::INFINITY), "Infinity");
        assert_eq!(number_to_string(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn number_to_string_shortest_round_trip() {
        assert_eq!(number_to_string(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(number_to_string(1.0 / 3.0), "0.3333333333333333");
        assert_eq!(number_to_string(100.0), "100");
        assert_eq!(number_to_string(-42.5), "-42.5");
        assert_eq!(
            number_to_string(123456789012345680000.0),
            "123456789012345680000"
        );
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(1e-6), "0.000001");
        assert_eq!(number_to_string(1e-7), "1e-7");
        assert_eq!(number_to_string(1.5e-7), "1.5e-7");
        assert_eq!(number_to_string(5e-324), "5e-324");
        assert_eq!(number_to_string(f64::MAX), "1.7976931348623157e+308");
    }

    #[test]
    fn to_boolean_falsy_and_truthy_values() {
        for falsy in [
            Value::Undefined,
            Value::Null,
            Value::Bool(false),
            Value::Number(0.0),
            Value::Number(-0.0),
            Value::Number(f64::NAN),
            Value::str(""),
            big(0),
        ] {
            assert!(!to_boolean(&falsy), "{falsy:?}");
        }
        for truthy in [
            Value::Bool(true),
            Value::Number(-1.0),
            Value::Number(f64::INFINITY),
            Value::str("0"),
            Value::str("false"),
            Value::str(" "),
            big(-1),
            Value::Symbol(SymbolId(0)),
            Value::Object(GcRef::new(0, 0)),
        ] {
            assert!(to_boolean(&truthy), "{truthy:?}");
        }
    }

    #[test]
    fn to_number_of_each_type() {
        let n = |v: Value| to_number(&v, &mut no_objects);
        assert!(n(Value::Undefined).unwrap().is_nan());
        assert_eq!(n(Value::Null).unwrap(), 0.0);
        assert_eq!(n(Value::Bool(true)).unwrap(), 1.0);
        assert_eq!(n(Value::Bool(false)).unwrap(), 0.0);
        assert!(n(Value::Number(-0.0)).unwrap().is_sign_negative());
        assert_eq!(n(Value::str(" 0x1F ")).unwrap(), 31.0);
        assert!(n(Value::Symbol(SymbolId(1))).is_err());
        assert!(n(big(1)).is_err());
    }

    #[test]
    fn to_number_converts_objects_through_the_hook_with_number_hint() {
        let mut seen = None;
        let mut hook = |r: GcRef, hint: Hint| {
            seen = Some((r, hint));
            Ok(Value::str("42"))
        };
        let result = to_number(&Value::Object(GcRef::new(7, 0)), &mut hook);
        assert_eq!(result, Ok(42.0));
        assert_eq!(seen, Some((GcRef::new(7, 0), Hint::Number)));
    }

    #[test]
    fn to_primitive_passes_primitives_through_and_rejects_object_results() {
        let prim = to_primitive(&Value::Number(1.0), Hint::Default, &mut no_objects);
        assert!(matches!(prim, Ok(Value::Number(n)) if n == 1.0));
        let mut returns_object = |r: GcRef, _| Ok(Value::Object(r));
        let result = to_primitive(
            &Value::Object(GcRef::new(1, 0)),
            Hint::String,
            &mut returns_object,
        );
        assert!(result.is_err());
    }

    #[test]
    fn to_string_of_each_type() {
        let t = |v: Value| to_string(&v, &mut no_objects).map(|s| s.to_string_lossy());
        assert_eq!(t(Value::Undefined).unwrap(), "undefined");
        assert_eq!(t(Value::Null).unwrap(), "null");
        assert_eq!(t(Value::Bool(true)).unwrap(), "true");
        assert_eq!(t(Value::Number(-0.0)).unwrap(), "0");
        assert_eq!(t(Value::Number(1e21)).unwrap(), "1e+21");
        assert_eq!(t(Value::str("x")).unwrap(), "x");
        assert_eq!(t(big(0)).unwrap(), "0");
        assert_eq!(t(big(-123)).unwrap(), "-123");
        let huge = Value::BigInt(Rc::new(JsBigInt::new(
            false,
            vec![0, 0, 0, 0, 0, 0, 0, 0, 1],
        )));
        assert_eq!(t(huge).unwrap(), "18446744073709551616");
        assert!(t(Value::Symbol(SymbolId(1))).is_err());
    }

    #[test]
    fn to_string_converts_objects_through_the_hook_with_string_hint() {
        let mut hint_seen = None;
        let mut hook = |_, hint| {
            hint_seen = Some(hint);
            Ok(Value::Number(1.5))
        };
        let result = to_string(&Value::Object(GcRef::new(0, 0)), &mut hook);
        assert_eq!(result.unwrap().to_string_lossy(), "1.5");
        assert_eq!(hint_seen, Some(Hint::String));
    }

    #[test]
    fn to_property_key_maps_strings_numbers_and_symbols() {
        let mut interner = Interner::default();
        let one = interner.intern_str("1");
        let key = |v: Value, i: &mut Interner| to_property_key(&v, i, &mut no_objects).unwrap();
        assert_eq!(
            key(Value::Number(1.0), &mut interner),
            PropertyKey::Atom(one)
        );
        assert_eq!(key(Value::str("1"), &mut interner), PropertyKey::Atom(one));
        assert_eq!(
            key(Value::Number(-0.0), &mut interner),
            PropertyKey::Atom(interner.intern_str("0"))
        );
        assert_eq!(
            key(Value::Symbol(SymbolId(9)), &mut interner),
            PropertyKey::Symbol(SymbolId(9))
        );
    }

    #[test]
    fn to_property_key_unwraps_object_symbols_via_the_hook() {
        let mut interner = Interner::default();
        let mut hook = |_, hint| {
            assert_eq!(hint, Hint::String);
            Ok(Value::Symbol(SymbolId(3)))
        };
        let result = to_property_key(&Value::Object(GcRef::new(0, 0)), &mut interner, &mut hook);
        assert_eq!(result, Ok(PropertyKey::Symbol(SymbolId(3))));
    }
}

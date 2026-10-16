//! Equality abstract operations (ECMA-262 7.2.10-7.2.15).

use std::mem::discriminant;

use velyr_engine_api::JsBigInt;

use crate::convert::{Hint, ObjectHook, is_js_whitespace, string_to_number, to_primitive};
use crate::value::{OpResult, Value};
use crate::{JsStr, bigint};

pub fn strict_equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Symbol(x), Value::Symbol(y)) => x == y,
        (Value::BigInt(x), Value::BigInt(y)) => x == y,
        (Value::Object(x), Value::Object(y)) => x == y,
        _ => false,
    }
}

pub fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan())
        }
        _ => strict_equals(a, b),
    }
}

pub fn same_value_zero(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || (x.is_nan() && y.is_nan()),
        _ => strict_equals(a, b),
    }
}

/// IsLooselyEqual (`==`). Objects are converted through `hook` with the default hint.
pub fn loose_equals(a: &Value, b: &Value, hook: ObjectHook) -> OpResult<bool> {
    if discriminant(a) == discriminant(b) {
        return Ok(strict_equals(a, b));
    }
    Ok(match (a, b) {
        (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null) => true,
        (Value::Number(n), Value::String(s)) | (Value::String(s), Value::Number(n)) => {
            *n == string_to_number(s)
        }
        (Value::BigInt(x), Value::String(s)) | (Value::String(s), Value::BigInt(x)) => {
            string_to_bigint(s).is_some_and(|y| **x == y)
        }
        (Value::BigInt(x), Value::Number(n)) | (Value::Number(n), Value::BigInt(x)) => {
            n.is_finite() && n.fract() == 0.0 && bigint::from_integral_f64(*n) == **x
        }
        (Value::Bool(x), _) => {
            return loose_equals(&Value::Number(f64::from(u8::from(*x))), b, hook);
        }
        (_, Value::Bool(y)) => {
            return loose_equals(a, &Value::Number(f64::from(u8::from(*y))), hook);
        }
        (
            Value::Object(_),
            Value::String(_) | Value::Number(_) | Value::BigInt(_) | Value::Symbol(_),
        ) => {
            return loose_equals(&to_primitive(a, Hint::Default, hook)?, b, hook);
        }
        (
            Value::String(_) | Value::Number(_) | Value::BigInt(_) | Value::Symbol(_),
            Value::Object(_),
        ) => {
            return loose_equals(a, &to_primitive(b, Hint::Default, hook)?, hook);
        }
        _ => false,
    })
}

/// StringToBigInt; `None` is the spec's `undefined`.
pub(crate) fn string_to_bigint(s: &JsStr) -> Option<JsBigInt> {
    bigint::parse(s.to_string_lossy().trim_matches(is_js_whitespace))
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use velyr_engine_api::JsBigInt;
    use velyr_engine_gc::GcRef;

    use super::*;
    use crate::{Hint, JsStr, SymbolId, TypeError, Value};

    fn no_objects(_: GcRef, _: Hint) -> Result<Value, TypeError> {
        panic!("hook must not be called")
    }

    fn big(n: i64) -> Value {
        Value::BigInt(Rc::new(JsBigInt::from_i64(n)))
    }

    fn loose(a: &Value, b: &Value) -> bool {
        loose_equals(a, b, &mut no_objects).unwrap()
    }

    fn n(x: f64) -> Value {
        Value::Number(x)
    }

    #[test]
    fn strict_equality_nan_and_signed_zero() {
        assert!(!strict_equals(&n(f64::NAN), &n(f64::NAN)));
        assert!(strict_equals(&n(0.0), &n(-0.0)));
        assert!(strict_equals(&n(1.0), &n(1.0)));
        assert!(!strict_equals(&n(1.0), &Value::str("1")));
        assert!(!strict_equals(&Value::Null, &Value::Undefined));
    }

    #[test]
    fn strict_equality_compares_strings_by_content_regardless_of_representation() {
        let rope = JsStr::from_str("0123456789").concat(&JsStr::from_str("abcdefghij"));
        let flat = Value::str("0123456789abcdefghij");
        assert!(strict_equals(&Value::String(rope), &flat));
        assert!(strict_equals(
            &Value::String(JsStr::from_utf16(vec![0x61])),
            &Value::str("a")
        ));
        assert!(!strict_equals(&Value::str("a"), &Value::str("A")));
    }

    #[test]
    fn strict_equality_objects_symbols_bigints() {
        assert!(strict_equals(
            &Value::Object(GcRef::new(1, 0)),
            &Value::Object(GcRef::new(1, 0))
        ));
        assert!(!strict_equals(
            &Value::Object(GcRef::new(1, 0)),
            &Value::Object(GcRef::new(1, 1))
        ));
        assert!(strict_equals(
            &Value::Symbol(SymbolId(1)),
            &Value::Symbol(SymbolId(1))
        ));
        assert!(!strict_equals(
            &Value::Symbol(SymbolId(1)),
            &Value::Symbol(SymbolId(2))
        ));
        assert!(strict_equals(&big(5), &big(5)));
        assert!(!strict_equals(&big(5), &n(5.0)));
    }

    #[test]
    fn same_value_distinguishes_signed_zero_and_equates_nan() {
        assert!(same_value(&n(f64::NAN), &n(f64::NAN)));
        assert!(!same_value(&n(0.0), &n(-0.0)));
        assert!(same_value(&n(-0.0), &n(-0.0)));
        assert!(same_value(&Value::str("a"), &Value::str("a")));
        assert!(!same_value(&n(1.0), &Value::Bool(true)));
    }

    #[test]
    fn same_value_zero_equates_nan_and_signed_zero() {
        assert!(same_value_zero(&n(f64::NAN), &n(f64::NAN)));
        assert!(same_value_zero(&n(0.0), &n(-0.0)));
        assert!(!same_value_zero(&n(1.0), &n(2.0)));
    }

    #[test]
    fn loose_equality_null_and_undefined() {
        assert!(loose(&Value::Null, &Value::Undefined));
        assert!(loose(&Value::Undefined, &Value::Undefined));
        assert!(!loose(&Value::Null, &n(0.0)));
        assert!(!loose(&Value::Undefined, &Value::str("")));
        assert!(!loose(&Value::Null, &Value::Bool(false)));
    }

    #[test]
    fn loose_equality_number_string_and_boolean_coercion() {
        assert!(loose(&n(12.0), &Value::str("  12  ")));
        assert!(loose(&n(0.0), &Value::str("")));
        assert!(loose(&n(31.0), &Value::str("0x1F")));
        assert!(!loose(&n(1000.0), &Value::str("1_000")));
        assert!(!loose(&n(f64::NAN), &Value::str("NaN")));
        assert!(loose(&Value::Bool(true), &n(1.0)));
        assert!(loose(&Value::Bool(false), &Value::str("0")));
        assert!(!loose(&Value::Bool(true), &Value::str("true")));
        assert!(loose(&Value::str("a"), &Value::str("a")));
    }

    #[test]
    fn loose_equality_bigint_with_number_and_string() {
        assert!(loose(&big(1), &n(1.0)));
        assert!(loose(&n(-3.0), &big(-3)));
        assert!(!loose(&big(1), &n(1.5)));
        assert!(!loose(&big(1), &n(f64::NAN)));
        assert!(!loose(&big(0), &n(f64::INFINITY)));
        assert!(loose(&big(0), &n(-0.0)));
        assert!(loose(&big(1), &Value::str(" 1 ")));
        assert!(loose(&big(0), &Value::str("")));
        assert!(!loose(&big(1), &Value::str("1.5")));
        assert!(!loose(&big(1), &Value::str("x")));
        let two_pow_64 = Value::BigInt(Rc::new(JsBigInt::new(
            false,
            vec![0, 0, 0, 0, 0, 0, 0, 0, 1],
        )));
        assert!(loose(&two_pow_64, &n(18446744073709551616.0)));
        assert!(!loose(&two_pow_64, &n(18446744073709555712.0)));
        assert!(loose(&big(1), &Value::Bool(true)));
    }

    #[test]
    fn loose_equality_converts_objects_with_default_hint() {
        let mut seen = None;
        let mut hook = |_, hint| {
            seen = Some(hint);
            Ok(Value::str("7"))
        };
        let obj = Value::Object(GcRef::new(0, 0));
        assert_eq!(loose_equals(&obj, &n(7.0), &mut hook), Ok(true));
        assert_eq!(seen, Some(Hint::Default));
        let mut hook = |_, _| Ok(Value::Number(1.0));
        assert_eq!(loose_equals(&Value::Bool(true), &obj, &mut hook), Ok(true));
    }

    #[test]
    fn loose_equality_object_vs_null_and_vs_object() {
        let obj = Value::Object(GcRef::new(0, 0));
        assert!(!loose(&obj, &Value::Null));
        assert!(loose(&obj, &obj));
        assert!(!loose(&obj, &Value::Object(GcRef::new(1, 0))));
    }

    #[test]
    fn loose_equality_symbol_only_equals_itself_or_its_wrapper() {
        assert!(loose(
            &Value::Symbol(SymbolId(1)),
            &Value::Symbol(SymbolId(1))
        ));
        assert!(!loose(&Value::Symbol(SymbolId(1)), &Value::str("Symbol()")));
    }
}

//! The `Number` constructor, its statics, its prototype and the global number functions
//! `parseInt`, `parseFloat`, `isNaN` and `isFinite` (ECMA-262 §21.1, §19.2).

use super::abstract_ops::MAX_SAFE_INTEGER;
use super::number_format::{Decimal, digits_text, exponential, shortest, to_radix_string};
use super::slots::Slot;
use super::{Interpreter, Intrinsic, JsResult, arg, object_value};

use crate::{Hint, Value, is_js_whitespace, number_to_string};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::NumberPrototype);
    vm.set_slot(proto, Slot::Number(0.0));
    let constructor = vm.constructor("Number", 1, number_constructor, proto)?;
    for (name, value) in [
        ("EPSILON", f64::EPSILON),
        ("MAX_SAFE_INTEGER", MAX_SAFE_INTEGER),
        ("MAX_VALUE", f64::MAX),
        ("MIN_SAFE_INTEGER", -MAX_SAFE_INTEGER),
        ("MIN_VALUE", f64::from_bits(1)),
        ("NaN", f64::NAN),
        ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
        ("POSITIVE_INFINITY", f64::INFINITY),
    ] {
        vm.constant(constructor, name, Value::Number(value))?;
    }
    vm.method(constructor, "isFinite", 1, number_is_finite)?;
    vm.method(constructor, "isInteger", 1, number_is_integer)?;
    vm.method(constructor, "isNaN", 1, number_is_nan)?;
    vm.method(constructor, "isSafeInteger", 1, number_is_safe_integer)?;
    let global = vm.state.intrinsics.global;
    vm.method(global, "isFinite", 1, global_is_finite)?;
    vm.method(global, "isNaN", 1, global_is_nan)?;
    for (name, arity, f) in [
        ("parseFloat", 1, parse_float as super::Builtin),
        ("parseInt", 2, parse_int),
    ] {
        let function = vm.new_builtin(name, arity, f)?;
        vm.define_method(global, name, function.clone())?;
        vm.define_method(constructor, name, function)?;
    }
    vm.method(proto, "toExponential", 1, number_to_exponential)?;
    vm.method(proto, "toFixed", 1, number_to_fixed)?;
    vm.method(proto, "toLocaleString", 0, number_to_locale_string)?;
    vm.method(proto, "toPrecision", 1, number_to_precision)?;
    vm.method(proto, "toString", 1, number_to_string_method)?;
    vm.method(proto, "valueOf", 0, number_value_of)
}

/// ToNumeric then Number(): BigInts convert (correctly rounded through their decimal text).
fn to_number_value(vm: &mut Interpreter, value: &Value) -> JsResult<f64> {
    let primitive = vm.coerce_primitive(value, Hint::Number)?;
    match &primitive {
        Value::BigInt(b) => Ok(crate::bigint::to_decimal_string(b)
            .parse()
            .unwrap_or(f64::NAN)),
        other => vm.coerce_number(other),
    }
}

fn number_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let n = match args.first() {
        Some(value) => to_number_value(vm, value)?,
        None => 0.0,
    };
    let new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        return Ok(Value::Number(n));
    }
    let proto = vm.intrinsic(Intrinsic::NumberPrototype);
    let object = vm.create_from_new_target(&new_target, proto, Some(Slot::Number(n)))?;
    Ok(object_value(object))
}

fn number_arg(args: &[Value]) -> Option<f64> {
    match args.first() {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

fn number_is_finite(_: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(number_arg(args).is_some_and(f64::is_finite)))
}

fn number_is_integer(_: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(
        number_arg(args).is_some_and(|n| n.is_finite() && n.trunc() == n),
    ))
}

fn number_is_nan(_: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(number_arg(args).is_some_and(f64::is_nan)))
}

fn number_is_safe_integer(_: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let safe = number_arg(args).is_some_and(|n| n.trunc() == n && n.abs() <= MAX_SAFE_INTEGER);
    Ok(Value::Bool(safe))
}

fn global_is_finite(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(vm.coerce_number(&arg(args, 0))?.is_finite()))
}

fn global_is_nan(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(vm.coerce_number(&arg(args, 0))?.is_nan()))
}

fn trim_start(text: &str) -> &str {
    text.trim_start_matches(is_js_whitespace)
}

fn parse_float(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let text = vm.coerce_rust_string(&arg(args, 0))?;
    let text = trim_start(&text);
    let bytes = text.as_bytes();
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    if text[end..].starts_with("Infinity") {
        let infinity = if bytes[0] == b'-' {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
        return Ok(Value::Number(infinity));
    }
    let digits_from = |mut i: usize| {
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        i
    };
    let integer_end = digits_from(end);
    let mut seen_digits = integer_end > end;
    end = integer_end;
    if bytes.get(end) == Some(&b'.') {
        let fraction_end = digits_from(end + 1);
        seen_digits |= fraction_end > end + 1;
        end = fraction_end;
    }
    if !seen_digits {
        return Ok(Value::Number(f64::NAN));
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let exponent_end = digits_from(end + 1 + sign);
        if exponent_end > end + 1 + sign {
            end = exponent_end;
        }
    }
    let literal = text[..end].trim_end_matches('.');
    Ok(Value::Number(literal.parse().unwrap_or(f64::NAN)))
}

fn parse_int(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let text = vm.coerce_rust_string(&arg(args, 0))?;
    let radix = crate::interpreter::operators::to_int32(vm.coerce_number(&arg(args, 1))?);
    let mut text = trim_start(&text);
    let negative = text.starts_with('-');
    if text.starts_with(['+', '-']) {
        text = &text[1..];
    }
    let mut radix = radix as u32;
    let mut strip_prefix = true;
    if radix != 0 {
        if !(2..=36).contains(&radix) {
            return Ok(Value::Number(f64::NAN));
        }
        strip_prefix = radix == 16;
    } else {
        radix = 10;
    }
    if strip_prefix && (text.starts_with("0x") || text.starts_with("0X")) {
        text = &text[2..];
        radix = 16;
    }
    let digits: Vec<u32> = text.chars().map_while(|c| c.to_digit(radix)).collect();
    if digits.is_empty() {
        return Ok(Value::Number(f64::NAN));
    }
    let magnitude = if radix == 10 {
        let literal: String = text.chars().take(digits.len()).collect();
        literal.parse().unwrap_or(f64::NAN)
    } else {
        // shortcut: accumulates in f64 (allowed as an approximation by the spec for
        // non-decimal radices); exact power-of-two rounding would need a bignum.
        digits
            .iter()
            .fold(0.0, |n, &d| n * f64::from(radix) + f64::from(d))
    };
    Ok(Value::Number(if negative { -magnitude } else { magnitude }))
}

/// thisNumberValue.
fn this_number(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<f64> {
    match this {
        Value::Number(n) => Ok(*n),
        _ => match vm.value_slot(this) {
            Some(Slot::Number(n)) => Ok(n),
            _ => vm.throw_type(&format!(
                "Number.prototype.{method} requires that 'this' be a Number"
            )),
        },
    }
}

/// A digits argument checked against `range` (after ToIntegerOrInfinity).
fn digits_arg(
    vm: &mut Interpreter,
    value: f64,
    range: std::ops::RangeInclusive<f64>,
    method: &str,
) -> JsResult<usize> {
    if !range.contains(&value) {
        return vm.throw_range(&format!(
            "{method}() argument must be between {} and 100",
            range.start()
        ));
    }
    Ok(value as usize)
}

fn signed(negative: bool, text: String) -> Value {
    Value::str(&if negative { format!("-{text}") } else { text })
}

fn number_to_fixed(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(vm, this, "toFixed")?;
    let f = vm.coerce_integer(&arg(args, 0))?;
    let f = digits_arg(vm, f, 0.0..=100.0, "toFixed")?;
    if !x.is_finite() || x.abs() >= 1e21 {
        return Ok(Value::str(&number_to_string(x)));
    }
    let mut digits = digits_text(&Decimal::exact(x.abs()).fixed(f));
    if digits.len() <= f {
        digits = format!("{}{digits}", "0".repeat(f + 1 - digits.len()));
    }
    if f > 0 {
        digits.insert(digits.len() - f, '.');
    }
    Ok(signed(x < 0.0, digits))
}

fn number_to_exponential(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(vm, this, "toExponential")?;
    let fraction = arg(args, 0);
    let f = vm.coerce_integer(&fraction)?;
    if !x.is_finite() {
        return Ok(Value::str(&number_to_string(x)));
    }
    let f = digits_arg(vm, f, 0.0..=100.0, "toExponential")?;
    let magnitude = x.abs();
    let (digits, exponent) = match (&fraction, magnitude == 0.0) {
        (_, true) => (vec![0; f + 1], 0),
        (Value::Undefined, false) => shortest(magnitude),
        (_, false) => Decimal::exact(magnitude).significant(f + 1),
    };
    Ok(signed(x < 0.0, exponential(&digits, exponent)))
}

fn number_to_precision(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(vm, this, "toPrecision")?;
    let precision = arg(args, 0);
    if matches!(precision, Value::Undefined) {
        return Ok(Value::str(&number_to_string(x)));
    }
    let p = vm.coerce_integer(&precision)?;
    if !x.is_finite() {
        return Ok(Value::str(&number_to_string(x)));
    }
    let p = digits_arg(vm, p, 1.0..=100.0, "toPrecision")?;
    let magnitude = x.abs();
    let (digits, e) = match magnitude == 0.0 {
        true => (vec![0; p], 0),
        false => Decimal::exact(magnitude).significant(p),
    };
    if e < -6 || e >= p as i32 {
        return Ok(signed(x < 0.0, exponential(&digits, e)));
    }
    let text = digits_text(&digits);
    let text = if e >= 0 {
        let split = e as usize + 1;
        match split < p {
            true => format!("{}.{}", &text[..split], &text[split..]),
            false => text,
        }
    } else {
        format!("0.{}{text}", "0".repeat((-e - 1) as usize))
    };
    Ok(signed(x < 0.0, text))
}

fn number_to_string_method(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(vm, this, "toString")?;
    let radix = match arg(args, 0) {
        Value::Undefined => 10.0,
        value => vm.coerce_integer(&value)?,
    };
    if !(2.0..=36.0).contains(&radix) {
        return vm.throw_range("toString() radix must be between 2 and 36");
    }
    if radix == 10.0 || !x.is_finite() {
        return Ok(Value::str(&number_to_string(x)));
    }
    if x == 0.0 {
        return Ok(Value::str("0"));
    }
    Ok(Value::str(&to_radix_string(x, radix as u32)))
}

/// shortcut: same output as `toString()`; locale-aware grouping waits for Intl.
fn number_to_locale_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let x = this_number(vm, this, "toLocaleString")?;
    Ok(Value::str(&number_to_string(x)))
}

fn number_value_of(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(this_number(vm, this, "valueOf")?))
}

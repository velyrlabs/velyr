//! The `BigInt` function, `BigInt.asIntN`/`asUintN` and `BigInt.prototype` (ECMA-262 §21.2).
//! The arithmetic lives in `crate::bigint_ops`.

use std::rc::Rc;

use velyr_engine_api::JsBigInt;

use super::abstract_ops::MAX_SAFE_INTEGER;
use super::slots::Slot;
use super::{ErrorKind, Interpreter, Intrinsic, JsResult, arg};
use crate::equality::string_to_bigint;
use crate::{Hint, Value, bigint, bigint_ops, number_to_string};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::BigIntPrototype);
    let constructor = vm.constructor("BigInt", 1, bigint_function, proto)?;
    vm.method(constructor, "asIntN", 2, as_int_n)?;
    vm.method(constructor, "asUintN", 2, as_uint_n)?;
    // Intl replaces toLocaleString with the locale-aware one.
    vm.method(proto, "toLocaleString", 0, bigint_to_locale_string)?;
    vm.method(proto, "toString", 0, bigint_to_string)?;
    vm.method(proto, "valueOf", 0, bigint_value_of)?;
    vm.define_to_string_tag(proto, "BigInt")
}

fn bigint_value(n: JsBigInt) -> Value {
    Value::BigInt(Rc::new(n))
}

/// `BigInt(value)`: NumberToBigInt for Numbers, else ToBigInt.
fn bigint_function(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    if !matches!(vm.new_target(), Value::Undefined) {
        return vm.throw_type("BigInt is not a constructor");
    }
    match vm.coerce_primitive(&arg(args, 0), Hint::Number)? {
        Value::Number(n) if n.is_finite() && n.trunc() == n => {
            Ok(bigint_value(bigint::from_integral_f64(n)))
        }
        Value::Number(n) => {
            let message = format!(
                "The number {} cannot be converted to a BigInt because it is not an integer",
                number_to_string(n)
            );
            Err(vm.error(ErrorKind::Range, &message))
        }
        primitive => to_bigint(vm, &primitive).map(Value::BigInt),
    }
}

/// ToBigInt (ECMA-262 §7.1.13).
pub(super) fn to_bigint(vm: &mut Interpreter, value: &Value) -> JsResult<Rc<JsBigInt>> {
    let primitive = vm.coerce_primitive(value, Hint::Number)?;
    let shown = match primitive {
        Value::BigInt(n) => return Ok(n),
        Value::Bool(b) => return Ok(Rc::new(JsBigInt::from_i64(i64::from(b)))),
        Value::String(text) => {
            if let Some(n) = string_to_bigint(&text) {
                return Ok(Rc::new(n));
            }
            let message = format!("Cannot convert {} to a BigInt", text.to_string_lossy());
            return Err(vm.error(ErrorKind::Syntax, &message));
        }
        Value::Number(n) => number_to_string(n),
        Value::Undefined => "undefined".into(),
        Value::Null => "null".into(),
        _ => "a Symbol".into(),
    };
    vm.throw_type(&format!("Cannot convert {shown} to a BigInt"))
}

/// ToIndex of the `bits` argument.
fn bits_arg(vm: &mut Interpreter, args: &[Value]) -> JsResult<u64> {
    let bits = vm.coerce_integer(&arg(args, 0))?;
    if !(0.0..=MAX_SAFE_INTEGER).contains(&bits) {
        let message = "Invalid value: not (convertible to) a safe integer";
        return Err(vm.error(ErrorKind::Range, message));
    }
    Ok(bits as u64)
}

fn as_int_n(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let bits = bits_arg(vm, args)?;
    let n = to_bigint(vm, &arg(args, 1))?;
    Ok(bigint_value(bigint_ops::as_int_n(bits, &n)))
}

fn as_uint_n(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let bits = bits_arg(vm, args)?;
    let n = to_bigint(vm, &arg(args, 1))?;
    match bigint_ops::as_uint_n(bits, &n, vm.bigint_max_bits) {
        Ok(wrapped) => Ok(bigint_value(wrapped)),
        Err(error) => Err(vm.bigint_error(error)),
    }
}

/// thisBigIntValue: a BigInt or a BigInt wrapper object.
fn this_bigint(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<Rc<JsBigInt>> {
    if let Value::BigInt(n) = this {
        return Ok(n.clone());
    }
    match vm.value_slot(this) {
        Some(Slot::BigInt(n)) => Ok(n),
        _ => vm.throw_type(&format!(
            "BigInt.prototype.{method} requires that 'this' be a BigInt"
        )),
    }
}

fn bigint_to_string(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let n = this_bigint(vm, this, "toString")?;
    let radix = match arg(args, 0) {
        Value::Undefined => 10.0,
        radix => vm.coerce_integer(&radix)?,
    };
    if !(2.0..=36.0).contains(&radix) {
        let message = "toString() radix must be between 2 and 36";
        return Err(vm.error(ErrorKind::Range, message));
    }
    Ok(Value::str(&bigint_ops::to_string_radix(&n, radix as u32)))
}

/// Without Intl the locale arguments are ignored (they are not a radix).
fn bigint_to_locale_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let n = this_bigint(vm, this, "toLocaleString")?;
    Ok(Value::str(&bigint_ops::to_string_radix(&n, 10)))
}

fn bigint_value_of(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    this_bigint(vm, this, "valueOf").map(Value::BigInt)
}

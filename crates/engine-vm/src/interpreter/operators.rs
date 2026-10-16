//! Operators and conversions. Objects reach a primitive through `coerce_primitive`, which runs
//! `valueOf`/`toString` in JS; the `convert` helpers then only ever see primitives.

use std::cmp::Ordering;
use std::rc::Rc;

use super::builtins::symbol::SYMBOL_TO_PRIMITIVE;
use super::dispatch::is_nullish;
use super::{ErrorKind, Interpreter, JsResult};
use crate::bigint_ops::{self, BigIntError};
use crate::equality::string_to_bigint;
use crate::{
    Hint, JsStr, ObjectKey, OpResult, Value, loose_equals, strict_equals, to_boolean, to_number,
    to_string,
};

const MIXED_BIGINT: &str = "Cannot mix BigInt and other types, use explicit conversions";

#[derive(Debug, Clone, Copy)]
pub(crate) enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Sar,
    Shr,
    Equal,
    NotEqual,
    StrictEqual,
    StrictNotEqual,
    LessThan,
    GreaterThan,
    LessThanOrEqual,
    GreaterThanOrEqual,
    In,
    InstanceOf,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum UnaryOp {
    Negate,
    BitNot,
    LogicalNot,
    TypeOf,
    ToNumber,
    ToNumeric,
    Inc,
    Dec,
    ToString,
    ToPropertyKey,
    CheckObjectCoercible,
}

/// `convert` hook for values already converted to primitives; never reached.
fn primitive_only(_: velyr_engine_gc::GcRef, _: Hint) -> OpResult<Value> {
    Err(crate::TypeError(
        "Cannot convert object to primitive value".into(),
    ))
}

impl Interpreter {
    pub(crate) fn binary(&mut self, op: BinaryOp, left: &Value, right: &Value) -> JsResult<Value> {
        use BinaryOp as B;
        match op {
            B::Add => self.add(left, right),
            B::StrictEqual => Ok(Value::Bool(strict_equals(left, right))),
            B::StrictNotEqual => Ok(Value::Bool(!strict_equals(left, right))),
            B::Equal => Ok(Value::Bool(self.loose_equal(left, right)?)),
            B::NotEqual => Ok(Value::Bool(!self.loose_equal(left, right)?)),
            B::LessThan => self.compare(left, right, true, |o| o == Some(Ordering::Less)),
            B::GreaterThan => self.compare(right, left, false, |o| o == Some(Ordering::Less)),
            B::LessThanOrEqual => self.compare(right, left, false, |o| {
                matches!(o, Some(Ordering::Greater | Ordering::Equal))
            }),
            B::GreaterThanOrEqual => self.compare(left, right, true, |o| {
                matches!(o, Some(Ordering::Greater | Ordering::Equal))
            }),
            B::In => self.has_property_op(left, right).map(Value::Bool),
            B::InstanceOf => self.instance_of(left, right).map(Value::Bool),
            _ => {
                let a = self.coerce_numeric(left)?;
                let b = self.coerce_numeric(right)?;
                self.numeric_binary(op, a, b)
            }
        }
    }

    /// A numeric operator on two ToNumeric results: both Numbers or both BigInts.
    fn numeric_binary(&mut self, op: BinaryOp, a: Value, b: Value) -> JsResult<Value> {
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => Ok(Value::Number(number_binary(op, a, b))),
            (Value::BigInt(a), Value::BigInt(b)) => {
                let result = bigint_ops::binary(op, &a, &b, self.bigint_max_bits);
                result
                    .map(|n| Value::BigInt(Rc::new(n)))
                    .map_err(|error| self.bigint_error(error))
            }
            _ => Err(self.error(ErrorKind::Type, MIXED_BIGINT)),
        }
    }

    pub(crate) fn bigint_error(&mut self, error: BigIntError) -> super::Abrupt {
        let kind = match error.is_type_error() {
            true => ErrorKind::Type,
            false => ErrorKind::Range,
        };
        self.error(kind, error.message())
    }

    pub(crate) fn unary(&mut self, op: UnaryOp, value: &Value) -> JsResult<Value> {
        use UnaryOp as U;
        Ok(match op {
            U::LogicalNot => Value::Bool(!to_boolean(value)),
            U::TypeOf => Value::str(self.type_of(value)),
            U::ToString => Value::String(self.coerce_string(value)?),
            U::ToPropertyKey => self.coerce_property_key(value)?,
            U::CheckObjectCoercible if is_nullish(value) => {
                let message = "Cannot destructure a null or undefined value";
                return Err(self.error(ErrorKind::Type, message));
            }
            U::CheckObjectCoercible => value.clone(),
            U::ToNumber => Value::Number(self.coerce_number(value)?),
            U::ToNumeric => self.coerce_numeric(value)?,
            U::Negate | U::BitNot | U::Inc | U::Dec => match self.coerce_numeric(value)? {
                Value::BigInt(n) => match bigint_ops::unary(op, &n, self.bigint_max_bits) {
                    Ok(result) => Value::BigInt(Rc::new(result)),
                    Err(error) => return Err(self.bigint_error(error)),
                },
                Value::Number(n) => Value::Number(match op {
                    U::Negate => -n,
                    U::BitNot => f64::from(!to_int32(n)),
                    U::Inc => n + 1.0,
                    _ => n - 1.0,
                }),
                _ => unreachable!("ToNumeric yields a Number or a BigInt"),
            },
        })
    }

    /// ToNumeric: a Number or a BigInt.
    pub(crate) fn coerce_numeric(&mut self, value: &Value) -> JsResult<Value> {
        match self.coerce_primitive(value, Hint::Number)? {
            bigint @ Value::BigInt(_) => Ok(bigint),
            primitive => Ok(Value::Number(self.coerce_number(&primitive)?)),
        }
    }

    /// ToPrimitive: `@@toPrimitive` when present, else OrdinaryToPrimitive
    /// (`valueOf`/`toString`, the other way round for the string hint).
    pub(crate) fn coerce_primitive(&mut self, value: &Value, hint: Hint) -> JsResult<Value> {
        let Some(object) = self.as_object(value) else {
            return Ok(value.clone());
        };
        let key = ObjectKey::Symbol(SYMBOL_TO_PRIMITIVE);
        if let Some(exotic) = self.get_method(value, key)? {
            let hint_name = match hint {
                Hint::Default => "default",
                Hint::Number => "number",
                Hint::String => "string",
            };
            let result = self.call_value(&exotic, value, &[Value::str(hint_name)])?;
            if matches!(result, Value::Object(_)) {
                return Err(self.error(ErrorKind::Type, "Cannot convert object to primitive value"));
            }
            return Ok(result);
        }
        let order = match hint {
            Hint::String => ["toString", "valueOf"],
            _ => ["valueOf", "toString"],
        };
        for name in order {
            let key = self.realm.key_str(name);
            let method = self.get(object, key, value)?;
            if self.callable(&method).is_some() {
                let result = self.call_value(&method, value, &[])?;
                if !matches!(result, Value::Object(_)) {
                    return Ok(result);
                }
            }
        }
        Err(self.error(ErrorKind::Type, "Cannot convert object to primitive value"))
    }

    pub(crate) fn coerce_string(&mut self, value: &Value) -> JsResult<JsStr> {
        let primitive = self.coerce_primitive(value, Hint::String)?;
        to_string(&primitive, &mut primitive_only).map_err(|e| self.error(ErrorKind::Type, &e.0))
    }

    /// ToNumber: a BigInt is a TypeError (use `coerce_numeric` where BigInts are allowed).
    pub(crate) fn coerce_number(&mut self, value: &Value) -> JsResult<f64> {
        let primitive = self.coerce_primitive(value, Hint::Number)?;
        to_number(&primitive, &mut primitive_only).map_err(|e| self.error(ErrorKind::Type, &e.0))
    }

    /// ToPropertyKey as a value: a String or a Symbol.
    pub(crate) fn coerce_property_key(&mut self, value: &Value) -> JsResult<Value> {
        match self.coerce_primitive(value, Hint::String)? {
            symbol @ Value::Symbol(_) => Ok(symbol),
            string @ Value::String(_) => Ok(string),
            other => Ok(Value::String(self.coerce_string(&other)?)),
        }
    }

    fn add(&mut self, left: &Value, right: &Value) -> JsResult<Value> {
        if let (Value::Number(a), Value::Number(b)) = (left, right) {
            return Ok(Value::Number(a + b));
        }
        let left = self.coerce_primitive(left, Hint::Default)?;
        let right = self.coerce_primitive(right, Hint::Default)?;
        if matches!(left, Value::String(_)) || matches!(right, Value::String(_)) {
            let left = self.coerce_string(&left)?;
            let right = self.coerce_string(&right)?;
            return Ok(Value::String(left.concat(&right)));
        }
        let a = self.coerce_numeric(&left)?;
        let b = self.coerce_numeric(&right)?;
        self.numeric_binary(BinaryOp::Add, a, b)
    }

    fn loose_equal(&mut self, left: &Value, right: &Value) -> JsResult<bool> {
        let (left, right) = match (left, right) {
            (Value::Object(_), Value::Object(_)) => return Ok(strict_equals(left, right)),
            (Value::Object(_), other) | (other, Value::Object(_)) if is_nullish(other) => {
                return Ok(false);
            }
            (Value::Object(_), _) => (self.coerce_primitive(left, Hint::Default)?, right.clone()),
            (_, Value::Object(_)) => (left.clone(), self.coerce_primitive(right, Hint::Default)?),
            _ => (left.clone(), right.clone()),
        };
        loose_equals(&left, &right, &mut primitive_only)
            .map_err(|e| self.error(ErrorKind::Type, &e.0))
    }

    /// IsLessThan(x, y) mapped through `accept`; `left_first` keeps ToPrimitive order.
    fn compare(
        &mut self,
        x: &Value,
        y: &Value,
        left_first: bool,
        accept: fn(Option<Ordering>) -> bool,
    ) -> JsResult<Value> {
        let (px, py) = if left_first {
            let px = self.coerce_primitive(x, Hint::Number)?;
            (px, self.coerce_primitive(y, Hint::Number)?)
        } else {
            let py = self.coerce_primitive(y, Hint::Number)?;
            (self.coerce_primitive(x, Hint::Number)?, py)
        };
        let ordering = match (&px, &py) {
            (Value::String(a), Value::String(b)) => Some(a.to_utf16().cmp(&b.to_utf16())),
            // A string that is not a BigInt literal compares as undefined (false).
            (Value::BigInt(a), Value::String(s)) => {
                string_to_bigint(s).map(|b| bigint_ops::compare(a, &b))
            }
            (Value::String(s), Value::BigInt(b)) => {
                string_to_bigint(s).map(|a| bigint_ops::compare(&a, b))
            }
            _ => match (self.coerce_numeric(&px)?, self.coerce_numeric(&py)?) {
                (Value::Number(a), Value::Number(b)) => a.partial_cmp(&b),
                (Value::BigInt(a), Value::BigInt(b)) => Some(bigint_ops::compare(&a, &b)),
                (Value::BigInt(a), Value::Number(b)) => bigint_ops::compare_with_number(&a, b),
                (Value::Number(a), Value::BigInt(b)) => {
                    bigint_ops::compare_with_number(&b, a).map(Ordering::reverse)
                }
                _ => unreachable!("ToNumeric yields a Number or a BigInt"),
            },
        };
        Ok(Value::Bool(accept(ordering)))
    }

    fn type_of(&self, value: &Value) -> &'static str {
        match value {
            Value::Undefined => "undefined",
            Value::Null => "object",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Symbol(_) => "symbol",
            Value::BigInt(_) => "bigint",
            Value::Object(_) if self.callable(value).is_some() => "function",
            Value::Object(_) => "object",
        }
    }

    /// `key in object`.
    fn has_property_op(&mut self, key: &Value, object: &Value) -> JsResult<bool> {
        let Some(o) = self.as_object(object) else {
            let message = "Cannot use 'in' operator to search for a key in a primitive";
            return Err(self.error(ErrorKind::Type, message));
        };
        let key = self.object_key(key)?;
        if super::class::is_private(key) {
            return self.private_has(o, key);
        }
        self.has_property(o, key)
    }

    fn instance_of(&mut self, value: &Value, target: &Value) -> JsResult<bool> {
        self.instance_of_operator(value, target)
    }
}

/// A numeric operator on two Numbers.
fn number_binary(op: BinaryOp, a: f64, b: f64) -> f64 {
    use BinaryOp as B;
    match op {
        B::Add => a + b,
        B::Sub => a - b,
        B::Mul => a * b,
        B::Div => a / b,
        B::Mod => a % b,
        B::Exp => exponentiate(a, b),
        B::BitAnd => f64::from(to_int32(a) & to_int32(b)),
        B::BitOr => f64::from(to_int32(a) | to_int32(b)),
        B::BitXor => f64::from(to_int32(a) ^ to_int32(b)),
        B::Shl => f64::from(to_int32(a).wrapping_shl(to_uint32(b))),
        B::Sar => f64::from(to_int32(a).wrapping_shr(to_uint32(b))),
        B::Shr => f64::from(to_uint32(a).wrapping_shr(to_uint32(b))),
        _ => unreachable!("{op:?} is not a numeric operator"),
    }
}

/// Number::exponentiate: unlike `powf`, `1 ** NaN` and `(-1) ** ±Infinity` are NaN.
pub(crate) fn exponentiate(base: f64, exponent: f64) -> f64 {
    if exponent.is_nan() || (base.abs() == 1.0 && exponent.is_infinite()) {
        return f64::NAN;
    }
    base.powf(exponent)
}

/// ToUint32 (ECMA-262 7.1.7): modulo 2^32 of the truncated value.
pub(crate) fn to_uint32(n: f64) -> u32 {
    if !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u32
}

pub(crate) fn to_int32(n: f64) -> i32 {
    to_uint32(n) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int32_conversions_wrap_modulo_two_to_the_32() {
        assert_eq!(to_int32(4_294_967_297.0), 1);
        assert_eq!(to_int32(-1.0), -1);
        assert_eq!(to_uint32(-1.0), u32::MAX);
        assert_eq!(to_int32(f64::NAN), 0);
        assert_eq!(to_int32(2_147_483_648.0), i32::MIN);
    }

    #[test]
    fn exponentiate_follows_number_semantics() {
        assert!(exponentiate(1.0, f64::NAN).is_nan());
        assert!(exponentiate(-1.0, f64::INFINITY).is_nan());
        assert_eq!(exponentiate(2.0, 10.0), 1024.0);
    }
}

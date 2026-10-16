//! BigInt operations (ECMA-262 §6.1.6.2) over `num-bigint`. Values stay `JsBigInt` (the
//! engine-api representation shared with the embedder) and convert per operation. A result
//! that would pass the bit limit is a `TooLarge` error found before allocating it, so
//! `2n ** 10000000n` under a small heap budget fails at once instead of exhausting memory.

use std::cmp::Ordering;

use num_bigint::{BigInt, Sign};
use num_traits::{FromPrimitive, One, Signed, ToPrimitive, Zero};
use velyr_engine_api::JsBigInt;

use crate::interpreter::operators::{BinaryOp, UnaryOp};

/// V8's `BigInt::kMaxLengthBits` on 64-bit hosts: the limit when there is no heap budget.
pub(crate) const DEFAULT_MAX_BITS: u64 = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BigIntError {
    DivisionByZero,
    NegativeExponent,
    TooLarge,
    UnsignedShift,
}

impl BigIntError {
    /// TypeError for `>>>`; every other failure is a RangeError.
    pub(crate) fn is_type_error(self) -> bool {
        self == Self::UnsignedShift
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::DivisionByZero => "Division by zero",
            Self::NegativeExponent => "Exponent must be non-negative",
            Self::TooLarge => "Maximum BigInt size exceeded",
            Self::UnsignedShift => "BigInts have no unsigned right shift, use >> instead",
        }
    }
}

pub(crate) type BigIntResult = Result<JsBigInt, BigIntError>;

fn to_big(value: &JsBigInt) -> BigInt {
    let sign = if value.is_negative() {
        Sign::Minus
    } else {
        Sign::Plus
    };
    BigInt::from_bytes_le(sign, value.magnitude_le())
}

fn from_big(value: &BigInt) -> JsBigInt {
    let (sign, magnitude) = value.to_bytes_le();
    JsBigInt::new(sign == Sign::Minus, magnitude)
}

fn within(bits: u64, max_bits: u64) -> Result<(), BigIntError> {
    if bits > max_bits {
        return Err(BigIntError::TooLarge);
    }
    Ok(())
}

/// An arithmetic, bitwise or shift `op` on two BigInts.
pub(crate) fn binary(op: BinaryOp, a: &JsBigInt, b: &JsBigInt, max_bits: u64) -> BigIntResult {
    let (x, y) = (to_big(a), to_big(b));
    let result = match op {
        BinaryOp::Add => x + y,
        BinaryOp::Sub => x - y,
        BinaryOp::Mul => {
            // A product has bits(x) + bits(y) - 1 or bits(x) + bits(y) bits.
            within((x.bits() + y.bits()).saturating_sub(1), max_bits)?;
            x * y
        }
        BinaryOp::Div | BinaryOp::Mod if y.is_zero() => return Err(BigIntError::DivisionByZero),
        BinaryOp::Div => x / y,
        BinaryOp::Mod => x % y,
        BinaryOp::Exp => exponentiate(&x, &y, max_bits)?,
        BinaryOp::BitAnd => x & y,
        BinaryOp::BitOr => x | y,
        BinaryOp::BitXor => x ^ y,
        BinaryOp::Shl => shift_left(&x, &y, max_bits)?,
        BinaryOp::Sar => shift_left(&x, &-y, max_bits)?,
        BinaryOp::Shr => return Err(BigIntError::UnsignedShift),
        _ => unreachable!("{op:?} is not a numeric operator"),
    };
    within(result.bits(), max_bits)?;
    Ok(from_big(&result))
}

fn exponentiate(base: &BigInt, exponent: &BigInt, max_bits: u64) -> Result<BigInt, BigIntError> {
    if exponent.is_negative() {
        return Err(BigIntError::NegativeExponent);
    }
    if exponent.is_zero() {
        return Ok(BigInt::one());
    }
    if base.is_zero() || base.abs().is_one() {
        let flips_sign = base.is_negative() && exponent.bit(0);
        return Ok(if flips_sign { base.clone() } else { base.abs() });
    }
    // |base| >= 2, so the power has at least (bits(base) - 1) * exponent + 1 bits.
    let exponent = exponent
        .to_u64()
        .filter(|&e| e <= max_bits)
        .ok_or(BigIntError::TooLarge)?;
    within((base.bits() - 1) * exponent + 1, max_bits)?;
    let exponent = u32::try_from(exponent).map_err(|_| BigIntError::TooLarge)?;
    Ok(base.pow(exponent))
}

/// `x << n` for any `n`; a negative `n` shifts right, rounding toward negative infinity.
fn shift_left(x: &BigInt, n: &BigInt, max_bits: u64) -> Result<BigInt, BigIntError> {
    if x.is_zero() {
        return Ok(BigInt::zero());
    }
    if n.is_negative() {
        let shift = n.magnitude().to_u64().unwrap_or(u64::MAX);
        if shift >= x.bits() {
            return Ok(if x.is_negative() {
                -BigInt::one()
            } else {
                BigInt::zero()
            });
        }
        return Ok(x >> shift);
    }
    let shift = n.to_u64().ok_or(BigIntError::TooLarge)?;
    within(x.bits().saturating_add(shift), max_bits)?;
    Ok(x << shift)
}

/// `-a`, `~a`, `a + 1n` or `a - 1n`.
pub(crate) fn unary(op: UnaryOp, a: &JsBigInt, max_bits: u64) -> BigIntResult {
    let x = to_big(a);
    let result = match op {
        UnaryOp::Negate => -x,
        UnaryOp::BitNot => !x,
        UnaryOp::Inc => x + 1,
        UnaryOp::Dec => x - 1,
        _ => unreachable!("{op:?} is not a numeric unary operator"),
    };
    within(result.bits(), max_bits)?;
    Ok(from_big(&result))
}

pub(crate) fn compare(a: &JsBigInt, b: &JsBigInt) -> Ordering {
    to_big(a).cmp(&to_big(b))
}

/// The exact order of a BigInt and a Number; `None` when the Number is NaN.
pub(crate) fn compare_with_number(a: &JsBigInt, n: f64) -> Option<Ordering> {
    if n.is_nan() {
        return None;
    }
    if n.is_infinite() {
        return Some(if n > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let floor = n.floor();
    let ordering = to_big(a).cmp(&BigInt::from_f64(floor)?);
    // Equal to floor(n) means below n when n has a fraction.
    Some(match ordering {
        Ordering::Equal if floor != n => Ordering::Less,
        ordering => ordering,
    })
}

/// `BigInt.asUintN(bits, a)`: `a` modulo 2^bits.
pub(crate) fn as_uint_n(bits: u64, a: &JsBigInt, max_bits: u64) -> BigIntResult {
    let x = to_big(a);
    if !x.is_negative() && x.bits() <= bits {
        return Ok(a.clone());
    }
    within(bits, max_bits)?;
    let mask = (BigInt::one() << bits) - 1;
    Ok(from_big(&(x & mask)))
}

/// `BigInt.asIntN(bits, a)`: `a` modulo 2^bits, read as a two's complement `bits`-bit value.
pub(crate) fn as_int_n(bits: u64, a: &JsBigInt) -> JsBigInt {
    let x = to_big(a);
    if bits == 0 {
        return JsBigInt::default();
    }
    if x.bits() < bits {
        return a.clone();
    }
    // Here bits <= bits(x), so the modulus is no larger than `a`.
    let modulus: BigInt = BigInt::one() << bits;
    let low: BigInt = x & (&modulus - 1);
    let wrapped = if low.bit(bits - 1) {
        low - modulus
    } else {
        low
    };
    from_big(&wrapped)
}

/// Number::toString-style digits in `radix` (2..=36), lowercase, `-` for negatives.
pub(crate) fn to_string_radix(a: &JsBigInt, radix: u32) -> String {
    to_big(a).to_str_radix(radix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big(n: i64) -> JsBigInt {
        JsBigInt::from_i64(n)
    }

    #[test]
    fn shifts_and_bitwise_use_twos_complement() {
        let shift = |op, a, b| binary(op, &big(a), &big(b), DEFAULT_MAX_BITS).unwrap();
        assert_eq!(shift(BinaryOp::Sar, -9, 1), big(-5));
        assert_eq!(shift(BinaryOp::Sar, -1, 1000), big(-1));
        assert_eq!(shift(BinaryOp::Shl, 3, -1), big(1));
        assert_eq!(shift(BinaryOp::BitAnd, -5, 0xff), big(251));
        assert_eq!(
            unary(UnaryOp::BitNot, &big(5), DEFAULT_MAX_BITS),
            Ok(big(-6))
        );
        // ~(2^3 - 1) = -2^3 needs 4 bits.
        assert_eq!(
            unary(UnaryOp::BitNot, &big(7), 3),
            Err(BigIntError::TooLarge)
        );
    }

    #[test]
    fn the_bit_limit_is_checked_before_computing() {
        let pow = |a, b, max| binary(BinaryOp::Exp, &big(a), &big(b), max);
        assert_eq!(pow(2, 10, 11), Ok(big(1024)));
        assert_eq!(pow(2, 11, 11), Err(BigIntError::TooLarge));
        assert_eq!(pow(-1, i64::MAX, 1), Ok(big(-1)));
        assert_eq!(pow(0, 0, 1), Ok(big(1)));
        assert_eq!(as_uint_n(64, &big(-1), 63), Err(BigIntError::TooLarge));
    }

    #[test]
    fn compares_with_fractional_and_infinite_numbers() {
        assert_eq!(compare_with_number(&big(-1), -0.5), Some(Ordering::Less));
        assert_eq!(compare_with_number(&big(2), 1.5), Some(Ordering::Greater));
        assert_eq!(
            compare_with_number(&big(1), f64::INFINITY),
            Some(Ordering::Less)
        );
        assert_eq!(compare_with_number(&big(1), f64::NAN), None);
        assert_eq!(as_int_n(8, &big(255)), big(-1));
        assert_eq!(as_int_n(8, &big(-128)), big(-128));
    }
}

//! Element types of TypedArrays and DataViews and their byte encodings (ECMA-262
//! §25.1.3.17 RawBytesToNumeric, §25.1.3.18 NumericToRawBytes). Elements travel as a raw
//! `u64` holding the element's bits, which is also what the Atomics operate on.

use std::rc::Rc;

use velyr_engine_api::JsBigInt;

use super::super::Intrinsic;
use crate::Value;
use crate::interpreter::operators::to_uint32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ElementKind {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float16,
    Float32,
    Float64,
    BigInt64,
    BigUint64,
}

impl ElementKind {
    pub(crate) const ALL: [ElementKind; 12] = [
        Self::Int8,
        Self::Uint8,
        Self::Uint8Clamped,
        Self::Int16,
        Self::Uint16,
        Self::Int32,
        Self::Uint32,
        Self::Float16,
        Self::Float32,
        Self::Float64,
        Self::BigInt64,
        Self::BigUint64,
    ];

    pub(crate) fn size(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 | Self::Uint8Clamped => 1,
            Self::Int16 | Self::Uint16 | Self::Float16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 | Self::BigInt64 | Self::BigUint64 => 8,
        }
    }

    /// The constructor name, `[[TypedArrayName]]`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8Array",
            Self::Uint8 => "Uint8Array",
            Self::Uint8Clamped => "Uint8ClampedArray",
            Self::Int16 => "Int16Array",
            Self::Uint16 => "Uint16Array",
            Self::Int32 => "Int32Array",
            Self::Uint32 => "Uint32Array",
            Self::Float16 => "Float16Array",
            Self::Float32 => "Float32Array",
            Self::Float64 => "Float64Array",
            Self::BigInt64 => "BigInt64Array",
            Self::BigUint64 => "BigUint64Array",
        }
    }

    pub(crate) fn is_bigint(self) -> bool {
        matches!(self, Self::BigInt64 | Self::BigUint64)
    }

    /// Integer element types other than `Uint8Clamped` (the ones Atomics accept).
    pub(crate) fn is_atomic(self) -> bool {
        !matches!(
            self,
            Self::Uint8Clamped | Self::Float16 | Self::Float32 | Self::Float64
        )
    }

    pub(crate) fn constructor(self) -> Intrinsic {
        match self {
            Self::Int8 => Intrinsic::Int8Array,
            Self::Uint8 => Intrinsic::Uint8Array,
            Self::Uint8Clamped => Intrinsic::Uint8ClampedArray,
            Self::Int16 => Intrinsic::Int16Array,
            Self::Uint16 => Intrinsic::Uint16Array,
            Self::Int32 => Intrinsic::Int32Array,
            Self::Uint32 => Intrinsic::Uint32Array,
            Self::Float16 => Intrinsic::Float16Array,
            Self::Float32 => Intrinsic::Float32Array,
            Self::Float64 => Intrinsic::Float64Array,
            Self::BigInt64 => Intrinsic::BigInt64Array,
            Self::BigUint64 => Intrinsic::BigUint64Array,
        }
    }

    /// The raw bits of `value`, already a Number (or a BigInt for BigInt kinds).
    pub(crate) fn encode(self, value: &Value) -> u64 {
        match (self, value) {
            (Self::BigInt64 | Self::BigUint64, Value::BigInt(n)) => bigint_bits(n),
            (_, Value::Number(n)) => number_bits(self, *n),
            _ => unreachable!("elements are coerced to the content type first"),
        }
    }

    pub(crate) fn decode(self, raw: u64) -> Value {
        let n = match self {
            Self::Int8 => f64::from(raw as u8 as i8),
            Self::Uint8 | Self::Uint8Clamped => f64::from(raw as u8),
            Self::Int16 => f64::from(raw as u16 as i16),
            Self::Uint16 => f64::from(raw as u16),
            Self::Int32 => f64::from(raw as u32 as i32),
            Self::Uint32 => f64::from(raw as u32),
            Self::Float16 => f16_to_f64(raw as u16),
            Self::Float32 => f64::from(f32::from_bits(raw as u32)),
            Self::Float64 => f64::from_bits(raw),
            Self::BigInt64 => return Value::BigInt(Rc::new(JsBigInt::from_i64(raw as i64))),
            Self::BigUint64 => {
                let magnitude = raw.to_le_bytes().to_vec();
                return Value::BigInt(Rc::new(JsBigInt::new(false, magnitude)));
            }
        };
        Value::Number(n)
    }

    /// Raw bits wrapped to the element width (the result of a modular Atomics operation).
    pub(crate) fn wrap(self, raw: u64) -> u64 {
        match self.size() {
            8 => raw,
            size => raw & ((1u64 << (size * 8)) - 1),
        }
    }
}

/// Reads a `src.len()`-byte element.
pub(crate) fn read_raw(src: &[u8], little_endian: bool) -> u64 {
    let mut bytes = [0u8; 8];
    if little_endian {
        bytes[..src.len()].copy_from_slice(src);
        u64::from_le_bytes(bytes)
    } else {
        bytes[8 - src.len()..].copy_from_slice(src);
        u64::from_be_bytes(bytes)
    }
}

/// Writes the low `dst.len()` bytes of `raw`.
pub(crate) fn write_raw(dst: &mut [u8], raw: u64, little_endian: bool) {
    let size = dst.len();
    if little_endian {
        dst.copy_from_slice(&raw.to_le_bytes()[..size]);
    } else {
        dst.copy_from_slice(&raw.to_be_bytes()[8 - size..]);
    }
}

fn number_bits(kind: ElementKind, n: f64) -> u64 {
    match kind {
        ElementKind::Uint8Clamped => {
            if n.is_nan() {
                0
            } else {
                n.clamp(0.0, 255.0).round_ties_even() as u64
            }
        }
        ElementKind::Float16 => u64::from(f64_to_f16(n)),
        ElementKind::Float32 => u64::from((n as f32).to_bits()),
        ElementKind::Float64 => n.to_bits(),
        // ToInt8..ToUint32 all keep the low bits of the value modulo 2^32.
        _ => u64::from(to_uint32(n)),
    }
}

/// The value modulo 2^64 (ToBigInt64 / ToBigUint64 bits).
fn bigint_bits(n: &JsBigInt) -> u64 {
    let mut bytes = [0u8; 8];
    let magnitude = n.magnitude_le();
    let low = magnitude.len().min(8);
    bytes[..low].copy_from_slice(&magnitude[..low]);
    let bits = u64::from_le_bytes(bytes);
    if n.is_negative() {
        bits.wrapping_neg()
    } else {
        bits
    }
}

/// IEEE 754 binary16 bits of `x`, rounded to nearest, ties to even (no double rounding).
pub(crate) fn f64_to_f16(x: f64) -> u16 {
    let sign = if x.is_sign_negative() { 0x8000 } else { 0 };
    if x.is_nan() {
        return 0x7e00;
    }
    let a = x.abs();
    if a >= 65520.0 {
        return sign | 0x7c00;
    }
    if a < 6.103_515_625e-5 {
        // Subnormal: a multiple of 2^-24; 1024 rounds up into the smallest normal.
        return sign | (a * 16_777_216.0).round_ties_even() as u16;
    }
    let exponent = ((a.to_bits() >> 52) & 0x7ff) as i32 - 1023;
    let fraction = (a / 2f64.powi(exponent) - 1.0) * 1024.0;
    let fraction = fraction.round_ties_even() as u16;
    // A fraction rounding up to 1024 carries into the exponent, as the encoding does.
    sign | ((((exponent + 15) as u16) << 10) + fraction)
}

pub(crate) fn f16_to_f64(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f64::from(bits & 0x3ff);
    match exponent {
        0 => sign * fraction * 2f64.powi(-24),
        31 if fraction == 0.0 => sign * f64::INFINITY,
        31 => f64::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2f64.powi(exponent - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float16_round_trips_and_rounds_to_nearest_even() {
        for bits in 0..=u16::MAX {
            let value = f16_to_f64(bits);
            if !value.is_nan() {
                assert_eq!(f64_to_f16(value), bits, "{bits:#x}");
            }
        }
        assert_eq!(f64_to_f16(2049.0), f64_to_f16(2048.0));
        assert_eq!(f64_to_f16(2051.0), f64_to_f16(2052.0));
        assert_eq!(f64_to_f16(65519.0), 0x7bff);
        assert_eq!(f64_to_f16(-1e-8), 0x8000);
        assert_eq!(f64_to_f16(f64::NEG_INFINITY), 0xfc00);
    }

    #[test]
    fn raw_bytes_follow_the_requested_byte_order() {
        let mut bytes = [0u8; 2];
        write_raw(&mut bytes, 0x1234, false);
        assert_eq!(bytes, [0x12, 0x34]);
        assert_eq!(read_raw(&bytes, true), 0x3412);
        let minus_one = JsBigInt::from_i64(-1);
        assert_eq!(
            ElementKind::BigInt64.encode(&Value::BigInt(Rc::new(minus_one))),
            u64::MAX
        );
    }
}

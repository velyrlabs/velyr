//! Helpers over `JsBigInt` (little-endian magnitude bytes): decimal output, string parsing, `f64` import.

use velyr_engine_api::JsBigInt;

pub fn to_decimal_string(value: &JsBigInt) -> String {
    crate::bigint_ops::to_string_radix(value, 10)
}

/// StringToBigInt on already-trimmed text: optional sign + decimal digits, or an unsigned
/// `0x`/`0o`/`0b` literal. `None` is the spec's `undefined`.
pub fn parse(text: &str) -> Option<JsBigInt> {
    if text.is_empty() {
        return Some(JsBigInt::default());
    }
    let (negative, rest) = match text.as_bytes()[0] {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let (radix, digits) = match rest.get(..2) {
        Some("0x" | "0X") if rest == text => (16, &rest[2..]),
        Some("0o" | "0O") if rest == text => (8, &rest[2..]),
        Some("0b" | "0B") if rest == text => (2, &rest[2..]),
        _ => (10, rest),
    };
    if digits.is_empty() {
        return None;
    }
    let mut magnitude: Vec<u8> = Vec::new();
    for c in digits.chars() {
        let mut carry = c.to_digit(radix)?;
        for byte in &mut magnitude {
            let current = u32::from(*byte) * radix + carry;
            *byte = current as u8;
            carry = current >> 8;
        }
        while carry > 0 {
            magnitude.push(carry as u8);
            carry >>= 8;
        }
    }
    Some(JsBigInt::new(negative, magnitude))
}

/// Exact conversion of an integral finite `f64`.
pub fn from_integral_f64(n: f64) -> JsBigInt {
    if n == 0.0 {
        return JsBigInt::default();
    }
    let bits = n.abs().to_bits();
    let mantissa = u128::from((bits & ((1 << 52) - 1)) | (1 << 52));
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1075;
    if exponent < 0 {
        return JsBigInt::new(n < 0.0, (mantissa >> -exponent).to_le_bytes().to_vec());
    }
    let shifted = (mantissa << (exponent % 8)).to_le_bytes();
    let mut magnitude = vec![0; (exponent / 8) as usize];
    magnitude.extend_from_slice(&shifted);
    JsBigInt::new(n < 0.0, magnitude)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_round_trip() {
        for text in [
            "0",
            "7",
            "-1",
            "255",
            "256",
            "18446744073709551616",
            "-123456789012345678901234567890",
        ] {
            assert_eq!(to_decimal_string(&parse(text).unwrap()), text);
        }
    }

    #[test]
    fn parse_radix_and_rejects() {
        assert_eq!(parse("0x10"), Some(JsBigInt::from_i64(16)));
        assert_eq!(parse("+5"), Some(JsBigInt::from_i64(5)));
        for bad in ["-0x10", "0x", "1.5", "1e3", "1n", "-", "x"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn imports_integral_floats_exactly() {
        assert_eq!(from_integral_f64(-3.0), JsBigInt::from_i64(-3));
        assert_eq!(from_integral_f64(-0.0), JsBigInt::default());
        assert_eq!(
            to_decimal_string(&from_integral_f64(1e21)),
            "1000000000000000000000"
        );
    }
}

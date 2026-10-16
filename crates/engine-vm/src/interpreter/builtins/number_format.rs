//! Exact Number formatting for `toFixed`, `toExponential`, `toPrecision` and radix
//! `toString` (ECMA-262 §21.1.3). Every finite double is a dyadic rational, so its full
//! decimal expansion is computed with a small bignum and rounded on the digits (ties
//! round up, as the spec's "pick the larger n" requires); no float formatting is used.

/// The exact decimal expansion of a finite, non-negative `x`: `digits` (no leading or
/// trailing zeros, empty for 0) with `x = 0.d1d2d3... * 10^point`.
pub(crate) struct Decimal {
    pub digits: Vec<u8>,
    pub point: i32,
}

/// Little-endian base-2^32 magnitude.
struct BigUint(Vec<u32>);

impl BigUint {
    fn from_u64(n: u64) -> Self {
        BigUint(vec![n as u32, (n >> 32) as u32])
    }

    fn mul_small(&mut self, factor: u32) {
        let mut carry = 0u64;
        for limb in &mut self.0 {
            let product = u64::from(*limb) * u64::from(factor) + carry;
            *limb = product as u32;
            carry = product >> 32;
        }
        if carry > 0 {
            self.0.push(carry as u32);
        }
    }

    fn is_zero(&self) -> bool {
        self.0.iter().all(|&limb| limb == 0)
    }

    /// Divides in place, returning the remainder.
    fn div_small(&mut self, divisor: u32) -> u32 {
        let mut remainder = 0u64;
        for limb in self.0.iter_mut().rev() {
            let current = (remainder << 32) | u64::from(*limb);
            *limb = (current / u64::from(divisor)) as u32;
            remainder = current % u64::from(divisor);
        }
        remainder as u32
    }

    /// Decimal digits, most significant first ("0" is empty).
    fn into_digits(mut self) -> Vec<u8> {
        let mut digits = Vec::new();
        while !self.is_zero() {
            let mut chunk = self.div_small(1_000_000_000);
            for _ in 0..9 {
                digits.push((chunk % 10) as u8);
                chunk /= 10;
            }
        }
        while digits.last() == Some(&0) {
            digits.pop();
        }
        digits.reverse();
        digits
    }
}

impl Decimal {
    pub(crate) fn exact(x: f64) -> Decimal {
        debug_assert!(x.is_finite() && x >= 0.0);
        if x == 0.0 {
            return Decimal {
                digits: Vec::new(),
                point: 0,
            };
        }
        let bits = x.to_bits();
        let biased = ((bits >> 52) & 0x7FF) as i32;
        let fraction = bits & ((1 << 52) - 1);
        let (mantissa, exponent) = match biased {
            0 => (fraction, -1074),
            _ => (fraction | (1 << 52), biased - 1075),
        };
        let mut big = BigUint::from_u64(mantissa);
        let scale = if exponent >= 0 {
            for _ in 0..exponent {
                big.mul_small(2);
            }
            0
        } else {
            for _ in 0..-exponent {
                big.mul_small(5);
            }
            -exponent
        };
        let mut digits = big.into_digits();
        let point = digits.len() as i32 - scale;
        while digits.last() == Some(&0) {
            digits.pop();
        }
        Decimal { digits, point }
    }

    /// Rounds to `count` significant digits (half up), padding with zeros; returns the
    /// digits and the exponent `e` with `x ~ d1.d2d3... * 10^e`.
    pub(crate) fn significant(&self, count: usize) -> (Vec<u8>, i32) {
        if self.digits.is_empty() {
            return (vec![0; count], 0);
        }
        let (digits, point) = round_at(&self.digits, self.point, count as i64);
        (digits, point - 1)
    }

    /// Digits of `round(x * 10^fraction)` as an integer (half up); empty for 0.
    pub(crate) fn fixed(&self, fraction: usize) -> Vec<u8> {
        let keep = i64::from(self.point) + fraction as i64;
        if keep < 0 {
            return Vec::new();
        }
        let (mut digits, point) = round_at(&self.digits, self.point, keep);
        let total = (i64::from(point) + fraction as i64) as usize;
        digits.resize(total, 0);
        digits
    }
}

/// Keeps the first `keep` digits of `0.digits * 10^point`, rounding half up and padding
/// with zeros; the returned point grows by one when rounding carries out.
fn round_at(digits: &[u8], point: i32, keep: i64) -> (Vec<u8>, i32) {
    let keep_len = keep.max(0) as usize;
    let mut kept: Vec<u8> = digits.iter().copied().take(keep_len).collect();
    let round_up = digits.get(keep_len).is_some_and(|&d| d >= 5);
    kept.resize(keep_len, 0);
    if !round_up {
        return (kept, point);
    }
    for digit in kept.iter_mut().rev() {
        if *digit == 9 {
            *digit = 0;
        } else {
            *digit += 1;
            return (kept, point);
        }
    }
    kept.insert(0, 1);
    kept.truncate(keep_len.max(1));
    (kept, point + 1)
}

pub(crate) fn digits_text(digits: &[u8]) -> String {
    digits.iter().map(|&d| char::from(b'0' + d)).collect()
}

/// `d1.d2d3...e±n` of ToString/toExponential.
pub(crate) fn exponential(digits: &[u8], exponent: i32) -> String {
    let mut out = digits_text(&digits[..1]);
    if digits.len() > 1 {
        out.push('.');
        out.push_str(&digits_text(&digits[1..]));
    }
    let sign = if exponent < 0 { '-' } else { '+' };
    out.push('e');
    out.push(sign);
    out.push_str(&exponent.unsigned_abs().to_string());
    out
}

/// The shortest round-trip digits of a finite, positive `x` and its exponent (ryu).
pub(crate) fn shortest(x: f64) -> (Vec<u8>, i32) {
    let text = crate::number_to_string(x);
    let (mantissa, exponent) = match text.split_once('e') {
        Some((m, e)) => (m.to_string(), e.parse::<i32>().unwrap_or(0)),
        None => (text.clone(), 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((&mantissa, ""));
    let all: Vec<u8> = integer
        .bytes()
        .chain(fraction.bytes())
        .map(|b| b - b'0')
        .collect();
    let leading = all.iter().take_while(|&&d| d == 0).count();
    let mut digits = all[leading..].to_vec();
    while digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
    }
    let exponent = exponent + integer.len() as i32 - 1 - leading as i32;
    (digits, exponent)
}

/// Number::toString(x, radix) for radix != 10, after V8's DoubleToRadixCString: the
/// fraction is emitted until it is within half an ulp, so the result round-trips.
pub(crate) fn to_radix_string(value: f64, radix: u32) -> String {
    const CHARS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let negative = value < 0.0;
    let value = value.abs();
    let mut integer = value.floor();
    let mut fraction = value - integer;
    let next_up = f64::from_bits(value.to_bits() + 1);
    let mut delta = (0.5 * (next_up - value)).max(f64::from_bits(1));
    let radix_f = f64::from(radix);
    let mut fraction_digits: Vec<u8> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= radix_f;
            delta *= radix_f;
            let digit = fraction as u32;
            fraction_digits.push(digit as u8);
            fraction -= f64::from(digit);
            if (fraction > 0.5 || (fraction == 0.5 && digit & 1 == 1)) && fraction + delta > 1.0 {
                loop {
                    match fraction_digits.pop() {
                        None => {
                            integer += 1.0;
                            break;
                        }
                        Some(d) if u32::from(d) + 1 < radix => {
                            fraction_digits.push(d + 1);
                            break;
                        }
                        Some(_) => {}
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }
    let mut integer_digits: Vec<u8> = Vec::new();
    while exponent_of(integer / radix_f) > 0 {
        integer /= radix_f;
        integer_digits.push(0);
    }
    loop {
        let remainder = integer % radix_f;
        integer_digits.push(remainder as u8);
        integer = (integer - remainder) / radix_f;
        if integer <= 0.0 {
            break;
        }
    }
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.extend(
        integer_digits
            .iter()
            .rev()
            .map(|&d| char::from(CHARS[d as usize])),
    );
    if !fraction_digits.is_empty() {
        out.push('.');
        out.extend(
            fraction_digits
                .iter()
                .map(|&d| char::from(CHARS[d as usize])),
        );
    }
    out
}

/// The binary exponent of `x` as `significand(53-bit integer) * 2^exponent`.
fn exponent_of(x: f64) -> i32 {
    let biased = ((x.to_bits() >> 52) & 0x7FF) as i32;
    if biased == 0 { -1074 } else { biased - 1075 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(x: f64, f: usize) -> String {
        digits_text(&Decimal::exact(x).fixed(f))
    }

    #[test]
    fn exact_expansion_is_exact() {
        let d = Decimal::exact(0.1);
        assert_eq!(&digits_text(&d.digits)[..20], "10000000000000000555");
        assert_eq!(d.point, 0);
        assert_eq!(digits_text(&Decimal::exact(1e21).digits), "1");
        assert_eq!(Decimal::exact(1e21).point, 22);
    }

    #[test]
    fn fixed_rounds_on_exact_digits() {
        assert_eq!(fixed(1.005, 2), "100");
        assert_eq!(fixed(1.255, 2), "125");
        assert_eq!(fixed(0.5, 0), "1");
        assert_eq!(fixed(2.5, 0), "3");
        assert_eq!(fixed(0.0001, 2), "");
        assert_eq!(fixed(999.99, 1), "10000");
    }

    #[test]
    fn significant_and_radix() {
        assert_eq!(Decimal::exact(123.456).significant(2), (vec![1, 2], 2));
        assert_eq!(Decimal::exact(9.99).significant(2), (vec![1, 0], 1));
        assert_eq!(shortest(1.5e-7), (vec![1, 5], -7));
        assert_eq!(shortest(120.0), (vec![1, 2], 2));
        assert_eq!(to_radix_string(255.5, 16), "ff.8");
        assert_eq!(to_radix_string(-0.5, 2), "-0.1");
    }
}

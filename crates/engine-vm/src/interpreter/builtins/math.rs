//! The `Math` namespace object (ECMA-262 §21.3), including `f16round` (ES2025) and
//! `sumPrecise` (stage 4).

use std::cell::Cell;

use super::{Interpreter, JsResult, arg, object_value};
use crate::Value;
use crate::interpreter::operators::{exponentiate, to_uint32};

type Unary = fn(f64) -> f64;

const UNARY: [(&str, Unary); 28] = [
    ("abs", f64::abs),
    ("acos", f64::acos),
    ("acosh", f64::acosh),
    ("asin", f64::asin),
    ("asinh", f64::asinh),
    ("atan", f64::atan),
    ("atanh", f64::atanh),
    ("cbrt", f64::cbrt),
    ("ceil", f64::ceil),
    ("cos", f64::cos),
    ("cosh", f64::cosh),
    ("exp", f64::exp),
    ("expm1", f64::exp_m1),
    ("f16round", f16_round),
    ("floor", f64::floor),
    ("fround", fround),
    ("log", f64::ln),
    ("log10", f64::log10),
    ("log1p", f64::ln_1p),
    ("log2", f64::log2),
    ("round", round),
    ("sign", sign),
    ("sin", f64::sin),
    ("sinh", f64::sinh),
    ("sqrt", f64::sqrt),
    ("tan", f64::tan),
    ("tanh", f64::tanh),
    ("trunc", f64::trunc),
];

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let math = vm.new_object()?;
    let global = vm.state.intrinsics.global;
    vm.define_method(global, "Math", object_value(math))?;
    vm.define_to_string_tag(math, "Math")?;
    for (name, value) in [
        ("E", std::f64::consts::E),
        ("LN10", std::f64::consts::LN_10),
        ("LN2", std::f64::consts::LN_2),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("PI", std::f64::consts::PI),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
        ("SQRT2", std::f64::consts::SQRT_2),
    ] {
        vm.constant(math, name, Value::Number(value))?;
    }
    for (index, (name, _)) in UNARY.iter().enumerate() {
        let function = vm.new_builtin(name, 1, unary_entry(index))?;
        vm.define_method(math, name, function)?;
    }
    vm.method(math, "atan2", 2, math_atan2)?;
    vm.method(math, "clz32", 1, math_clz32)?;
    vm.method(math, "hypot", 2, math_hypot)?;
    vm.method(math, "imul", 2, math_imul)?;
    vm.method(math, "max", 2, math_max)?;
    vm.method(math, "min", 2, math_min)?;
    vm.method(math, "pow", 2, math_pow)?;
    vm.method(math, "random", 0, math_random)?;
    vm.method(math, "sumPrecise", 1, math_sum_precise)?;
    Ok(())
}

/// A `fn` pointer per unary entry (built-ins are plain `fn`s, so no closures).
fn unary_entry(index: usize) -> super::Builtin {
    macro_rules! entries {
        ($($i:literal)*) => {
            [$(|vm: &mut Interpreter, _: &Value, args: &[Value]| {
                let x = vm.coerce_number(&arg(args, 0))?;
                Ok(Value::Number((UNARY[$i].1)(x)))
            }),*]
        };
    }
    const TABLE: [super::Builtin; 28] =
        entries!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27);
    TABLE[index]
}

fn fround(x: f64) -> f64 {
    f64::from(x as f32)
}

fn sign(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        x
    } else {
        x.signum()
    }
}

/// Math.round: ties toward +Infinity, keeping -0 for `-0.5 <= x < 0`.
fn round(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    if (-0.5..0.0).contains(&x) {
        return -0.0;
    }
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// Rounds to the nearest IEEE 754 binary16 value (ties to even), straight from f64.
fn f16_round(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let magnitude = x.abs();
    if magnitude >= 65520.0 {
        return f64::INFINITY.copysign(x);
    }
    let exponent = ((magnitude.to_bits() >> 52) as i32 - 1023).max(-14);
    let quantum = 2f64.powi(exponent - 10);
    ((magnitude / quantum).round_ties_even() * quantum).copysign(x)
}

fn numbers(vm: &mut Interpreter, args: &[Value]) -> JsResult<Vec<f64>> {
    args.iter().map(|value| vm.coerce_number(value)).collect()
}

fn math_atan2(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let y = vm.coerce_number(&arg(args, 0))?;
    let x = vm.coerce_number(&arg(args, 1))?;
    Ok(Value::Number(y.atan2(x)))
}

fn math_pow(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let base = vm.coerce_number(&arg(args, 0))?;
    let exponent = vm.coerce_number(&arg(args, 1))?;
    Ok(Value::Number(exponentiate(base, exponent)))
}

fn math_clz32(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let n = to_uint32(vm.coerce_number(&arg(args, 0))?);
    Ok(Value::Number(f64::from(n.leading_zeros())))
}

fn math_imul(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let a = to_uint32(vm.coerce_number(&arg(args, 0))?);
    let b = to_uint32(vm.coerce_number(&arg(args, 1))?);
    Ok(Value::Number(f64::from(a.wrapping_mul(b) as i32)))
}

fn math_max(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let values = numbers(vm, args)?;
    let result = values.into_iter().fold(f64::NEG_INFINITY, |a, b| {
        if a.is_nan() || b.is_nan() {
            f64::NAN
        } else if a == b && a == 0.0 {
            if a.is_sign_positive() { a } else { b }
        } else {
            a.max(b)
        }
    });
    Ok(Value::Number(result))
}

fn math_min(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let values = numbers(vm, args)?;
    let result = values.into_iter().fold(f64::INFINITY, |a, b| {
        if a.is_nan() || b.is_nan() {
            f64::NAN
        } else if a == b && a == 0.0 {
            if a.is_sign_negative() { a } else { b }
        } else {
            a.min(b)
        }
    });
    Ok(Value::Number(result))
}

fn math_hypot(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let values = numbers(vm, args)?;
    if values.iter().any(|v| v.is_infinite()) {
        return Ok(Value::Number(f64::INFINITY));
    }
    if values.iter().any(|v| v.is_nan()) {
        return Ok(Value::Number(f64::NAN));
    }
    let largest = values.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if largest == 0.0 {
        return Ok(Value::Number(0.0));
    }
    let sum: f64 = values.iter().map(|v| (v / largest) * (v / largest)).sum();
    Ok(Value::Number(sum.sqrt() * largest))
}

thread_local! {
    static RANDOM_STATE: Cell<u64> = Cell::new(random_seed());
}

fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let local = 0u8;
    (nanos ^ (&local as *const u8 as u64).rotate_left(32)) | 1
}

/// xorshift64*: not cryptographic (the spec only asks for a uniform distribution).
fn math_random(_: &mut Interpreter, _: &Value, _: &[Value]) -> JsResult<Value> {
    let bits = RANDOM_STATE.with(|state| {
        let mut x = state.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        state.set(x);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    });
    Ok(Value::Number((bits >> 11) as f64 / (1u64 << 53) as f64))
}

/// Number of 64-bit limbs of the exact accumulator: 2^-1074 .. 2^1024 plus headroom
/// for 2^53 carries (`sumPrecise` stops at 2^53 values).
const LIMBS: usize = 36;

/// Fixed-point magnitude in units of 2^-1074.
struct Exact([u64; LIMBS]);

impl Exact {
    fn add_shifted(&mut self, mantissa: u64, shift: u32) {
        let limb = (shift / 64) as usize;
        let bit = shift % 64;
        let wide = u128::from(mantissa) << bit;
        let mut carry = 0u128;
        for (offset, part) in [wide as u64, (wide >> 64) as u64].into_iter().enumerate() {
            let sum = u128::from(self.0[limb + offset]) + u128::from(part) + carry;
            self.0[limb + offset] = sum as u64;
            carry = sum >> 64;
        }
        let mut index = limb + 2;
        while carry > 0 {
            let sum = u128::from(self.0[index]) + carry;
            self.0[index] = sum as u64;
            carry = sum >> 64;
            index += 1;
        }
    }

    fn sub(&self, other: &Exact) -> Exact {
        let mut out = [0u64; LIMBS];
        let mut borrow = false;
        for (index, slot) in out.iter_mut().enumerate() {
            let (d, b1) = self.0[index].overflowing_sub(other.0[index]);
            let (d, b2) = d.overflowing_sub(u64::from(borrow));
            *slot = d;
            borrow = b1 || b2;
        }
        Exact(out)
    }

    fn at_least(&self, other: &Exact) -> bool {
        self.0.iter().rev().ge(other.0.iter().rev())
    }

    fn bit(&self, index: usize) -> bool {
        self.0[index / 64] >> (index % 64) & 1 == 1
    }

    /// Correctly rounded (ties to even) value of `self * 2^-1074`.
    fn to_f64(&self) -> f64 {
        let Some(top) = (0..LIMBS * 64).rev().find(|&i| self.bit(i)) else {
            return 0.0;
        };
        if top < 53 {
            return (self.0[0] as f64) * f64::from_bits(1);
        }
        let low = top - 52;
        let mut mantissa = (low..=top)
            .rev()
            .fold(0u64, |m, i| (m << 1) | u64::from(self.bit(i)));
        let half = self.bit(low - 1);
        let sticky = (0..low - 1).any(|i| self.bit(i));
        if half && (sticky || mantissa & 1 == 1) {
            mantissa += 1;
        }
        let mut exponent = low as i32 - 1074;
        if mantissa == 1 << 53 {
            mantissa >>= 1;
            exponent += 1;
        }
        if exponent > 971 {
            return f64::INFINITY;
        }
        // Normal result (top >= 53): biased exponent = exponent + 1075.
        f64::from_bits(((exponent + 1075) as u64) << 52 | (mantissa - (1 << 52)))
    }
}

fn math_sum_precise(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let items = arg(args, 0);
    vm.require_coercible(&items, "Math.sumPrecise")?;
    let (mut positive, mut negative) = (Exact([0; LIMBS]), Exact([0; LIMBS]));
    let (mut has_nan, mut pos_inf, mut neg_inf, mut all_negative_zero) =
        (false, false, false, true);
    let mut count = 0u64;
    vm.iterate(&items, |vm, value| {
        let Value::Number(x) = value else {
            return vm.throw_type("Math.sumPrecise: every value must be a Number");
        };
        count += 1;
        if count >= 1 << 53 {
            return vm.throw_range("Math.sumPrecise: too many values");
        }
        if !(x == 0.0 && x.is_sign_negative()) {
            all_negative_zero = false;
        }
        match x {
            x if x.is_nan() => has_nan = true,
            f64::INFINITY => pos_inf = true,
            f64::NEG_INFINITY => neg_inf = true,
            x if x != 0.0 => {
                let bits = x.to_bits();
                let biased = ((bits >> 52) & 0x7FF) as u32;
                let fraction = bits & ((1 << 52) - 1);
                let (mantissa, shift) = match biased {
                    0 => (fraction, 0),
                    _ => (fraction | (1 << 52), biased - 1),
                };
                let target = if x > 0.0 {
                    &mut positive
                } else {
                    &mut negative
                };
                target.add_shifted(mantissa, shift);
            }
            _ => {}
        }
        Ok(())
    })?;
    let result = if has_nan || (pos_inf && neg_inf) {
        f64::NAN
    } else if pos_inf {
        f64::INFINITY
    } else if neg_inf {
        f64::NEG_INFINITY
    } else if all_negative_zero {
        -0.0
    } else if positive.at_least(&negative) {
        positive.sub(&negative).to_f64()
    } else {
        -negative.sub(&positive).to_f64()
    };
    Ok(Value::Number(result))
}

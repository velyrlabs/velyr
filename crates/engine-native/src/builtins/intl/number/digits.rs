//! The digit options of `Intl.NumberFormat` and `Intl.PluralRules` (ECMA-402
//! `SetNumberFormatDigitOptions`): rounding mode, increment, priority and trailing zeros.

use fixed_decimal::{Decimal, RoundingIncrement, SignedRoundingMode, UnsignedRoundingMode};

/// Option names in the order the prelude joins them into the spec.
pub(super) const KEYS: [&str; 16] = [
    "minimumIntegerDigits",
    "minimumFractionDigits",
    "maximumFractionDigits",
    "minimumSignificantDigits",
    "maximumSignificantDigits",
    "useGrouping",
    "style",
    "currency",
    "currencyDisplay",
    "currencySign",
    "notation",
    "signDisplay",
    "roundingMode",
    "roundingIncrement",
    "roundingPriority",
    "trailingZeroDisplay",
];

pub(super) struct Spec(pub(super) Vec<String>);

impl Spec {
    pub(super) fn parse(text: &str) -> Self {
        Self(text.split(',').map(str::to_owned).collect())
    }

    pub(super) fn text(&self, key: &str) -> &str {
        let index = KEYS.iter().position(|k| *k == key);
        index.and_then(|i| self.0.get(i)).map_or("", String::as_str)
    }

    fn digits(&self, key: &str) -> Option<i16> {
        self.text(key).parse().ok()
    }

    fn pair(&self, min: &str, max: &str, min_default: i16) -> Option<(i16, i16)> {
        let max = self.digits(max)?;
        Some((self.digits(min).unwrap_or(min_default), max))
    }
}

fn rounding_mode(name: &str) -> SignedRoundingMode {
    use SignedRoundingMode::{Ceil, Floor, HalfCeil, HalfFloor, Unsigned};
    match name {
        "ceil" => Ceil,
        "floor" => Floor,
        "expand" => Unsigned(UnsignedRoundingMode::Expand),
        "trunc" => Unsigned(UnsignedRoundingMode::Trunc),
        "halfCeil" => HalfCeil,
        "halfFloor" => HalfFloor,
        "halfTrunc" => Unsigned(UnsignedRoundingMode::HalfTrunc),
        "halfEven" => Unsigned(UnsignedRoundingMode::HalfEven),
        _ => Unsigned(UnsignedRoundingMode::HalfExpand),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Priority {
    Auto,
    MorePrecision,
    LessPrecision,
}

pub(super) struct Digits {
    min_integer: i16,
    fraction: Option<(i16, i16)>,
    significant: Option<(i16, i16)>,
    priority: Priority,
    mode: SignedRoundingMode,
    /// `(increment, power of ten)`: the spec increment is `increment * 10^power`.
    increment: (RoundingIncrement, i16),
    strip_integer_zeros: bool,
}

/// Splits a validated spec increment (1, 2, 5, 10 .. 5000) into a base and a power of ten.
fn increment(value: u16) -> (RoundingIncrement, i16) {
    let (mut base, mut power) = (value, 0);
    while base % 10 == 0 {
        base /= 10;
        power += 1;
    }
    let base = match base {
        2 => RoundingIncrement::MultiplesOf2,
        5 => RoundingIncrement::MultiplesOf5,
        25 => RoundingIncrement::MultiplesOf25,
        _ => RoundingIncrement::MultiplesOf1,
    };
    (base, power)
}

impl Digits {
    pub(super) fn from_spec(spec: &Spec) -> Self {
        let significant = spec.pair("minimumSignificantDigits", "maximumSignificantDigits", 1);
        let mut fraction = spec.pair("minimumFractionDigits", "maximumFractionDigits", 0);
        if fraction.is_none() && significant.is_none() {
            fraction = Some((0, 3));
        }
        let priority = match spec.text("roundingPriority") {
            "morePrecision" => Priority::MorePrecision,
            "lessPrecision" => Priority::LessPrecision,
            _ => Priority::Auto,
        };
        Self {
            min_integer: spec.digits("minimumIntegerDigits").unwrap_or(1),
            fraction,
            significant,
            priority,
            mode: rounding_mode(spec.text("roundingMode")),
            increment: increment(spec.digits("roundingIncrement").unwrap_or(1) as u16),
            strip_integer_zeros: spec.text("trailingZeroDisplay") == "stripIfInteger",
        }
    }

    pub(super) fn apply(&self, decimal: &mut Decimal) {
        match (self.fraction, self.significant) {
            (Some(fraction), Some(significant)) => {
                let mut by_fraction = decimal.clone();
                self.round_fraction(&mut by_fraction, fraction);
                self.round_significant(decimal, significant);
                // A lower rounding magnitude keeps more digits.
                let significant_wins = (magnitude_of(decimal, significant.1) <= -fraction.1)
                    == (self.priority == Priority::MorePrecision);
                if !significant_wins {
                    *decimal = by_fraction;
                }
            }
            (Some(fraction), None) => self.round_fraction(decimal, fraction),
            (None, Some(significant)) => self.round_significant(decimal, significant),
            (None, None) => {}
        }
        decimal.absolute.pad_start(self.min_integer);
        if self.strip_integer_zeros {
            decimal.absolute.trim_end_if_integer();
        }
    }

    fn round_fraction(&self, decimal: &mut Decimal, (min, max): (i16, i16)) {
        let (increment, power) = self.increment;
        decimal.round_with_mode_and_increment(power - max, self.mode, increment);
        decimal.absolute.trim_end();
        decimal.absolute.pad_end(-min);
    }

    fn round_significant(&self, decimal: &mut Decimal, (min, max): (i16, i16)) {
        if !decimal.absolute.is_zero() {
            let top = decimal.absolute.nonzero_magnitude_start();
            decimal.round_with_mode(top - max + 1, self.mode);
            decimal.absolute.trim_end();
        }
        // Rounding can carry into a new leading digit (999.96 -> 1000).
        let top = decimal.absolute.nonzero_magnitude_start();
        decimal.absolute.pad_end(top - min + 1);
    }
}

/// The magnitude of the last significant digit a `max` digit rounding left in `decimal`.
fn magnitude_of(decimal: &Decimal, max: i16) -> i16 {
    if decimal.absolute.is_zero() {
        return 1 - max;
    }
    decimal.absolute.nonzero_magnitude_start() - max + 1
}

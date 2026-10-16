//! `Intl.NumberFormat` and `Intl.PluralRules` over ICU4X `DecimalFormatter` and
//! `PluralRules`. The prelude resolves the options into a comma separated spec (see
//! `Spec`); rounding is `halfExpand`. Currency and percent affixes live in `currency.rs`.

use fixed_decimal::{
    Decimal, FloatPrecision, Sign, SignDisplay, SignedRoundingMode, UnsignedRoundingMode,
};
use icu_decimal::options::{DecimalFormatterOptions, GroupingStrategy};
use icu_decimal::{DecimalFormatter, parts};
use icu_plurals::{PluralCategory, PluralRuleType, PluralRules, PluralRulesOptions};
use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};
use writeable::{Part, Writeable};

use super::currency::{self, Layout, NBSP};
use super::{OpResult, Parts, bool_arg, locale, number_arg, range_error, string_arg};

/// Option names in the order the prelude joins them into the spec.
const KEYS: [&str; 12] = [
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
];

struct Spec(Vec<String>);

impl Spec {
    fn text(&self, key: &str) -> &str {
        let index = KEYS.iter().position(|k| *k == key);
        index.and_then(|i| self.0.get(i)).map_or("", String::as_str)
    }

    fn digits(&self, key: &str) -> Option<i16> {
        self.text(key).parse().ok()
    }
}

enum Precision {
    Fraction { min: i16, max: i16 },
    Significant { min: i16, max: i16 },
}

struct Digits {
    min_integer: i16,
    precision: Precision,
}

impl Digits {
    fn fraction(min_integer: i16, min: i16, max: i16) -> Self {
        Self {
            min_integer,
            precision: Precision::Fraction { min, max },
        }
    }

    fn from_spec(spec: &Spec) -> Self {
        let min_integer = spec.digits("minimumIntegerDigits").unwrap_or(1);
        let precision = match spec.digits("maximumSignificantDigits") {
            Some(max) => Precision::Significant {
                min: spec.digits("minimumSignificantDigits").unwrap_or(1),
                max,
            },
            None => Precision::Fraction {
                min: spec.digits("minimumFractionDigits").unwrap_or(0),
                max: spec.digits("maximumFractionDigits").unwrap_or(3),
            },
        };
        Self {
            min_integer,
            precision,
        }
    }

    fn apply(&self, decimal: &mut Decimal) {
        let mode = SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand);
        match self.precision {
            Precision::Fraction { min, max } => {
                decimal.round_with_mode(-max, mode);
                decimal.absolute.trim_end();
                decimal.absolute.pad_end(-min);
            }
            Precision::Significant { min, max } => {
                if !decimal.absolute.is_zero() {
                    let top = decimal.absolute.nonzero_magnitude_start();
                    decimal.round_with_mode(top - max + 1, mode);
                    decimal.absolute.trim_end();
                }
                // Rounding can carry into a new leading digit (999.96 -> 1000).
                let top = decimal.absolute.nonzero_magnitude_start();
                decimal.absolute.pad_end(top - min + 1);
            }
        }
        decimal.absolute.pad_start(self.min_integer);
    }
}

/// A number as a decimal: a JS number, or the decimal string of a BigInt.
fn decimal_of(value: &Value) -> Option<Decimal> {
    match value {
        Value::Number(n) => Decimal::try_from_f64(*n, FloatPrecision::RoundTrip).ok(),
        Value::String(s) => Decimal::try_from_str(&s.to_string_lossy()).ok(),
        _ => None,
    }
}

fn part_name(part: Part) -> &'static str {
    match part {
        p if p == parts::INTEGER => "integer",
        p if p == parts::FRACTION => "fraction",
        p if p == parts::GROUP => "group",
        p if p == parts::DECIMAL => "decimal",
        p if p == parts::MINUS_SIGN => "minusSign",
        p if p == parts::PLUS_SIGN => "plusSign",
        _ => "literal",
    }
}

fn grouping(name: &str) -> GroupingStrategy {
    match name {
        "false" => GroupingStrategy::Never,
        "always" => GroupingStrategy::Always,
        "min2" => GroupingStrategy::Min2,
        _ => GroupingStrategy::Auto,
    }
}

fn sign_display(name: &str) -> SignDisplay {
    match name {
        "never" => SignDisplay::Never,
        "always" => SignDisplay::Always,
        "exceptZero" => SignDisplay::ExceptZero,
        "negative" => SignDisplay::Negative,
        _ => SignDisplay::Auto,
    }
}

/// Moves the decimal point of `decimal` to the mantissa of scientific (one integer digit)
/// or engineering (a multiple of three exponent) notation and returns the exponent.
fn to_mantissa(decimal: &mut Decimal, step: i16) -> i16 {
    let top = if decimal.absolute.is_zero() {
        0
    } else {
        decimal.absolute.nonzero_magnitude_start()
    };
    let exponent = top - top.rem_euclid(step);
    decimal.absolute.multiply_pow10(-exponent);
    exponent
}

/// Rounds the mantissa, renormalizing when rounding carries (9.99 -> 10.0).
fn scientific_mantissa(decimal: &mut Decimal, digits: &Digits, step: i16) -> i16 {
    let mut exponent = to_mantissa(decimal, step);
    digits.apply(decimal);
    if !decimal.absolute.is_zero() && decimal.absolute.nonzero_magnitude_start() >= step {
        decimal.absolute.multiply_pow10(-step);
        exponent += step;
        digits.apply(decimal);
    }
    exponent
}

fn exponent_parts(exponent: i16) -> [(&'static str, String); 3] {
    let minus = if exponent < 0 { "-" } else { "" };
    [
        ("exponentSeparator", "E".into()),
        ("exponentMinusSign", minus.into()),
        ("exponentInteger", exponent.unsigned_abs().to_string()),
    ]
}

type Entries = Vec<(&'static str, String)>;

fn non_finite(n: f64, display: SignDisplay) -> Entries {
    let mut out = Entries::new();
    if n.is_nan() {
        out.push(("nan", "NaN".into()));
        return out;
    }
    let mut sign = Decimal::from(1u8);
    sign.sign = if n < 0.0 { Sign::Negative } else { Sign::None };
    sign.apply_sign_display(display);
    match sign.sign {
        Sign::Negative => out.push(("minusSign", "-".into())),
        Sign::Positive => out.push(("plusSign", "+".into())),
        Sign::None => {}
    }
    out.push(("infinity", "\u{221e}".into()));
    out
}

/// The number part of the output: digits, grouping and sign, plus the exponent.
fn number_entries(
    vm: &mut Interpreter,
    spec: &Spec,
    value: &Value,
    tag: &str,
) -> Result<Entries, Abrupt> {
    let display = sign_display(spec.text("signDisplay"));
    if let Value::Number(n) = value
        && !n.is_finite()
    {
        return Ok(non_finite(*n, display));
    }
    let Some(mut decimal) = decimal_of(value) else {
        return Err(range_error(vm, "Invalid number"));
    };
    if spec.text("style") == "percent" {
        decimal.absolute.multiply_pow10(2);
    }
    let digits = Digits::from_spec(spec);
    let notation = spec.text("notation");
    let exponent = match notation {
        "scientific" => Some(scientific_mantissa(&mut decimal, &digits, 1)),
        "engineering" => Some(scientific_mantissa(&mut decimal, &digits, 3)),
        _ => {
            digits.apply(&mut decimal);
            None
        }
    };
    decimal.apply_sign_display(display);
    let options = DecimalFormatterOptions::from(grouping(spec.text("useGrouping")));
    let Ok(formatter) = DecimalFormatter::try_new((&locale(tag)).into(), options) else {
        return Err(range_error(vm, "No number data for this locale"));
    };
    let mut out = Parts::new(part_name);
    let _ = formatter.format(&decimal).write_to_parts(&mut out);
    let mut entries = out.parts;
    if let Some(exponent) = exponent {
        entries.extend(
            exponent_parts(exponent)
                .into_iter()
                .filter(|(_, text)| !text.is_empty()),
        );
    }
    Ok(entries)
}

/// Adds the percent sign or currency symbol around `entries` per the locale's layout.
fn add_affixes(spec: &Spec, tag: &str, entries: &mut Entries) {
    let layout = Layout::of(&locale(tag));
    match spec.text("style") {
        "percent" => {
            if layout.percent_space {
                entries.push(("literal", NBSP.into()));
            }
            entries.push(("percentSign", "%".into()));
        }
        "currency" => add_currency(spec, &layout, entries),
        _ => {}
    }
}

fn add_currency(spec: &Spec, layout: &Layout, entries: &mut Entries) {
    let symbol = layout.symbol(spec.text("currency"), spec.text("currencyDisplay"));
    if layout.currency_after {
        entries.push(("literal", NBSP.into()));
        entries.push(("currency", symbol));
        return;
    }
    let sign = entries.first().map(|(kind, _)| *kind);
    let head = usize::from(matches!(sign, Some("minusSign" | "plusSign")));
    let accounting = sign == Some("minusSign") && spec.text("currencySign") == "accounting";
    if accounting {
        entries[0] = ("literal", "(".into());
    }
    if layout.spaces_prefix(&symbol) {
        entries.insert(head, ("literal", NBSP.into()));
    }
    entries.insert(head, ("currency", symbol));
    if accounting {
        entries.push(("literal", ")".into()));
    }
}

/// `(locale, value, spec, parts)`: the formatted string, or encoded parts when `parts` is
/// true.
pub(crate) fn format(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let spec = Spec(string_arg(args, 2).split(',').map(str::to_owned).collect());
    let value = args.get(1).cloned().unwrap_or(Value::Undefined);
    let tag = string_arg(args, 0);
    let mut entries = number_entries(vm, &spec, &value, &tag)?;
    add_affixes(&spec, &tag, &mut entries);
    if bool_arg(args, 3) {
        let encoded: Vec<_> = entries
            .iter()
            .map(|(kind, text)| format!("{kind}\u{1}{text}"))
            .collect();
        return Ok(Value::str(&encoded.join("\u{2}")));
    }
    let text: String = entries.iter().map(|(_, text)| text.as_str()).collect();
    Ok(Value::str(&text))
}

/// `(code)`: the fraction digits of a currency.
pub(crate) fn currency_digits(_: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    Ok(Value::Number(f64::from(currency::fraction_digits(
        &string_arg(args, 0),
    ))))
}

fn rules(vm: &mut Interpreter, tag: &str, ordinal: bool) -> Result<PluralRules, Abrupt> {
    let rule_type = if ordinal {
        PluralRuleType::Ordinal
    } else {
        PluralRuleType::Cardinal
    };
    let options = PluralRulesOptions::default().with_type(rule_type);
    PluralRules::try_new((&locale(tag)).into(), options)
        .map_err(|_| range_error(vm, "No plural data for this locale"))
}

fn category_name(category: PluralCategory) -> &'static str {
    match category {
        PluralCategory::Zero => "zero",
        PluralCategory::One => "one",
        PluralCategory::Two => "two",
        PluralCategory::Few => "few",
        PluralCategory::Many => "many",
        PluralCategory::Other => "other",
    }
}

/// `(locale, ordinal, value, minFrac, maxFrac)`: the plural category of the value as it
/// would be formatted.
pub(crate) fn plural_select(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let rules = rules(vm, &string_arg(args, 0), bool_arg(args, 1))?;
    let n = number_arg(args, 2);
    if !n.is_finite() {
        return Ok(Value::str("other"));
    }
    let mut decimal = decimal_of(&Value::Number(n)).unwrap_or_default();
    let digit = |i: usize, fallback: f64| {
        let n = number_arg(args, i);
        (if n.is_nan() { fallback } else { n }) as i16
    };
    Digits::fraction(1, digit(3, 0.0), digit(4, 3.0)).apply(&mut decimal);
    Ok(Value::str(category_name(rules.category_for(&decimal))))
}

/// `(locale, ordinal)`: the categories of the locale, comma separated, in CLDR order.
pub(crate) fn plural_categories(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let rules = rules(vm, &string_arg(args, 0), bool_arg(args, 1))?;
    let names: Vec<_> = rules.categories().map(category_name).collect();
    Ok(Value::str(&names.join(",")))
}

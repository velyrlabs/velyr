//! `Intl.NumberFormat` and `Intl.PluralRules` over ICU4X `DecimalFormatter` and
//! `PluralRules`. The prelude resolves the digit options; rounding is `halfExpand`.

use fixed_decimal::{Decimal, FloatPrecision, SignedRoundingMode, UnsignedRoundingMode};
use icu_decimal::options::{DecimalFormatterOptions, GroupingStrategy};
use icu_decimal::{DecimalFormatter, parts};
use icu_plurals::{PluralCategory, PluralRuleType, PluralRules, PluralRulesOptions};
use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};
use writeable::{Part, Writeable};

use super::{OpResult, Parts, bool_arg, locale, number_arg, range_error, string_arg};

/// minimumIntegerDigits, minimumFractionDigits, maximumFractionDigits.
struct Digits {
    min_integer: i16,
    min_fraction: i16,
    max_fraction: i16,
}

impl Digits {
    fn from_args(args: &[Value], at: usize) -> Self {
        let digits = |i: usize, fallback: f64| {
            let n = number_arg(args, at + i);
            (if n.is_nan() { fallback } else { n }) as i16
        };
        Self {
            min_integer: digits(0, 1.0),
            min_fraction: digits(1, 0.0),
            max_fraction: digits(2, 3.0),
        }
    }

    fn apply(&self, decimal: &mut Decimal) {
        let mode = SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand);
        decimal.round_with_mode(-self.max_fraction, mode);
        decimal.absolute.trim_end();
        decimal.absolute.pad_end(-self.min_fraction);
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

/// `(locale, value, minInt, minFrac, maxFrac, grouping, style, parts)`: the formatted
/// string, or encoded parts when `parts` is true.
/// shortcut: `style: 'percent'` appends `%` (right for en and pt, not for de/fr's
/// spaced sign); use ICU4X's percent formatter once it leaves icu_experimental.
pub(crate) fn format(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let value = args.get(1).cloned().unwrap_or(Value::Undefined);
    let percent = string_arg(args, 6) == "percent";
    let mut out = Parts::new(part_name);
    match value {
        Value::Number(n) if n.is_nan() => out.parts.push(("nan", "NaN".into())),
        Value::Number(n) if n.is_infinite() => {
            if n < 0.0 {
                out.parts.push(("minusSign", "-".into()));
            }
            out.parts.push(("infinity", "∞".into()));
        }
        _ => {
            let Some(mut decimal) = decimal_of(&value) else {
                return Err(range_error(vm, "Invalid number"));
            };
            if percent {
                decimal.multiply_pow10(2);
            }
            Digits::from_args(args, 2).apply(&mut decimal);
            let options = DecimalFormatterOptions::from(grouping(&string_arg(args, 5)));
            let prefs = (&locale(&string_arg(args, 0))).into();
            let Ok(formatter) = DecimalFormatter::try_new(prefs, options) else {
                return Err(range_error(vm, "No number data for this locale"));
            };
            let _ = formatter.format(&decimal).write_to_parts(&mut out);
        }
    }
    if percent {
        out.parts.push(("percentSign", "%".into()));
    }
    if bool_arg(args, 7) {
        return Ok(Value::str(&out.encode()));
    }
    let text: String = out.parts.iter().map(|(_, text)| text.as_str()).collect();
    Ok(Value::str(&text))
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
    let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
    let digits = [Value::Number(1.0), arg(3), arg(4)];
    Digits::from_args(&digits, 0).apply(&mut decimal);
    Ok(Value::str(category_name(rules.category_for(&decimal))))
}

/// `(locale, ordinal)`: the categories of the locale, comma separated, in CLDR order.
pub(crate) fn plural_categories(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let rules = rules(vm, &string_arg(args, 0), bool_arg(args, 1))?;
    let names: Vec<_> = rules.categories().map(category_name).collect();
    Ok(Value::str(&names.join(",")))
}

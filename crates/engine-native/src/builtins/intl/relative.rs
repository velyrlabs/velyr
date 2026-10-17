//! `Intl.RelativeTimeFormat` ops: ICU4X `icu_experimental::relativetime` with
//! `intl-experimental`, otherwise an `en`/`pt` table (`relative_table.rs`). The
//! number is rounded to three fraction digits and formatted by `icu_decimal` in both paths.

use fixed_decimal::{Decimal, FloatPrecision, Sign, SignedRoundingMode, UnsignedRoundingMode};
use icu_decimal::parts;
use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::{Interpreter, NativeFn};
use writeable::Part;

use super::{OpResult, Parts, number_arg, range_error, string_arg};

pub(super) type RelativeParts = Parts<fn(Part) -> &'static str>;

/// Maximum fraction digits of the default `Intl.NumberFormat`, which formats the number.
const MAX_FRACTION_DIGITS: i16 = 3;

pub(super) fn ops() -> Vec<(&'static str, u32, NativeFn)> {
    vec![("__intl_relative_time", 5, std::rc::Rc::new(format))]
}

pub(super) fn part_name(part: Part) -> &'static str {
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

pub(super) fn new_parts() -> RelativeParts {
    Parts::new(part_name as fn(Part) -> &'static str)
}

/// `__intl_relative_time(locale, style, numeric, value, unit)` -> encoded parts; `unit` is
/// singular and validated by the prelude.
fn format(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let (tag, style, numeric, unit) = (
        string_arg(args, 0),
        string_arg(args, 1),
        string_arg(args, 2),
        string_arg(args, 4),
    );
    let value = number_arg(args, 3);
    // A named phrase ("yesterday") only for exactly -1, 0 or 1, never for a rounded value.
    let auto = numeric == "auto" && matches!(value, -1.0 | 0.0 | 1.0);
    let Ok(mut decimal) = Decimal::try_from_f64(value, FloatPrecision::RoundTrip) else {
        return Err(range_error(vm, "Invalid time value"));
    };
    if !auto {
        let mode = SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand);
        decimal.round_with_mode(-MAX_FRACTION_DIGITS, mode);
        decimal.absolute.trim_end();
    }
    let Some(mut out) = relative_parts(&tag, &style, &unit, auto, decimal) else {
        return Err(range_error(vm, "No relative time data for this locale"));
    };
    out.parts.retain(|(_, text)| !text.is_empty());
    Ok(Value::str(&out.encode()))
}

/// The digits of `decimal` (its magnitude) as number parts.
pub(super) fn number_parts(tag: &str, decimal: &Decimal) -> Option<RelativeParts> {
    use icu_decimal::DecimalFormatter;
    use writeable::Writeable;

    let formatter = DecimalFormatter::try_new((&super::locale(tag)).into(), Default::default());
    let mut out = new_parts();
    let magnitude = decimal.clone().with_sign(Sign::None);
    let _ = formatter.ok()?.format(&magnitude).write_to_parts(&mut out);
    Some(out)
}

#[cfg(feature = "intl-experimental")]
/// `text` as literal parts around the first occurrence of the number `number`.
pub(super) fn split_around(text: &str, number: RelativeParts) -> RelativeParts {
    let digits: String = number.parts.iter().map(|(_, part)| part.as_str()).collect();
    let mut out = new_parts();
    let (before, after) = match text.split_once(&digits) {
        Some(halves) if !digits.is_empty() => halves,
        _ => return literal(text),
    };
    out.parts.push(("literal", before.to_owned()));
    out.parts.extend(number.parts);
    out.parts.push(("literal", after.to_owned()));
    out
}

pub(super) fn literal(text: &str) -> RelativeParts {
    let mut out = new_parts();
    out.parts.push(("literal", text.to_owned()));
    out
}

#[cfg(feature = "intl-experimental")]
fn relative_parts(
    tag: &str,
    style: &str,
    unit: &str,
    auto: bool,
    decimal: Decimal,
) -> Option<RelativeParts> {
    use icu_experimental::relativetime::options::Numeric;
    use icu_experimental::relativetime::{RelativeTimeFormatter, RelativeTimeFormatterOptions};
    let mut options = RelativeTimeFormatterOptions::default();
    options.numeric = if auto { Numeric::Auto } else { Numeric::Always };
    let prefs = (&super::locale(tag)).into();
    let formatter = match (style, unit) {
        ("long", "second") => RelativeTimeFormatter::try_new_long_second(prefs, options),
        ("long", "minute") => RelativeTimeFormatter::try_new_long_minute(prefs, options),
        ("long", "hour") => RelativeTimeFormatter::try_new_long_hour(prefs, options),
        ("long", "day") => RelativeTimeFormatter::try_new_long_day(prefs, options),
        ("long", "week") => RelativeTimeFormatter::try_new_long_week(prefs, options),
        ("long", "month") => RelativeTimeFormatter::try_new_long_month(prefs, options),
        ("long", "quarter") => RelativeTimeFormatter::try_new_long_quarter(prefs, options),
        ("long", "year") => RelativeTimeFormatter::try_new_long_year(prefs, options),
        ("short", "second") => RelativeTimeFormatter::try_new_short_second(prefs, options),
        ("short", "minute") => RelativeTimeFormatter::try_new_short_minute(prefs, options),
        ("short", "hour") => RelativeTimeFormatter::try_new_short_hour(prefs, options),
        ("short", "day") => RelativeTimeFormatter::try_new_short_day(prefs, options),
        ("short", "week") => RelativeTimeFormatter::try_new_short_week(prefs, options),
        ("short", "month") => RelativeTimeFormatter::try_new_short_month(prefs, options),
        ("short", "quarter") => RelativeTimeFormatter::try_new_short_quarter(prefs, options),
        ("short", "year") => RelativeTimeFormatter::try_new_short_year(prefs, options),
        ("narrow", "second") => RelativeTimeFormatter::try_new_narrow_second(prefs, options),
        ("narrow", "minute") => RelativeTimeFormatter::try_new_narrow_minute(prefs, options),
        ("narrow", "hour") => RelativeTimeFormatter::try_new_narrow_hour(prefs, options),
        ("narrow", "day") => RelativeTimeFormatter::try_new_narrow_day(prefs, options),
        ("narrow", "week") => RelativeTimeFormatter::try_new_narrow_week(prefs, options),
        ("narrow", "month") => RelativeTimeFormatter::try_new_narrow_month(prefs, options),
        ("narrow", "quarter") => RelativeTimeFormatter::try_new_narrow_quarter(prefs, options),
        ("narrow", "year") => RelativeTimeFormatter::try_new_narrow_year(prefs, options),
        _ => return None,
    }
    .ok()?;
    // ICU writes the pattern without nested parts, so the number is located in the text.
    let text = formatter.format(decimal.clone()).to_string();
    let number = number_parts(tag, &decimal)?;
    Some(split_around(&text, number))
}

#[cfg(not(feature = "intl-experimental"))]
fn relative_parts(
    tag: &str,
    style: &str,
    unit: &str,
    auto: bool,
    decimal: Decimal,
) -> Option<RelativeParts> {
    table::parts(tag, style, unit, auto, decimal)
}

#[cfg(not(feature = "intl-experimental"))]
#[path = "relative_table.rs"]
mod table;

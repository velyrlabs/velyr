//! Ops behind `Intl.supportedValuesOf` and the week info and time zone members of
//! `Intl.Locale` (`prelude_enum.js`). Lists travel as `\u{1}`-joined strings.

use std::rc::Rc;

use icu_calendar::preferences::WeekPreferences;
use icu_calendar::week::WeekInformation;
use icu_locale::Locale;
use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::{Interpreter, NativeFn};

use super::{OpResult, string_arg};
use crate::date::parse_zone;

const SEPARATOR: &str = "\u{1}";
/// `CC zone` lines of the tzdb `zone.tab`: the zones in common use in each region.
const ZONES: &str = include_str!("zones.tab");

/// ECMA-402 table of sanctioned simple unit identifiers.
const UNITS: [&str; 45] = [
    "acre",
    "bit",
    "byte",
    "celsius",
    "centimeter",
    "day",
    "degree",
    "fahrenheit",
    "fluid-ounce",
    "foot",
    "gallon",
    "gigabit",
    "gigabyte",
    "gram",
    "hectare",
    "hour",
    "inch",
    "kilobit",
    "kilobyte",
    "kilogram",
    "kilometer",
    "liter",
    "megabit",
    "megabyte",
    "meter",
    "microsecond",
    "mile",
    "mile-scandinavian",
    "milliliter",
    "millimeter",
    "millisecond",
    "minute",
    "month",
    "nanosecond",
    "ounce",
    "percent",
    "petabyte",
    "pound",
    "second",
    "stone",
    "terabit",
    "terabyte",
    "week",
    "yard",
    "year",
];

/// The ISO 4217 codes in circulation. `NumberFormat` accepts any well-formed code, so this
/// is only the list advertised by `supportedValuesOf`.
const CURRENCIES: &str = "AED AFN ALL AMD ANG AOA ARS AUD AWG AZN BAM BBD BDT BGN BHD BIF BMD BND BOB \
BRL BSD BTN BWP BYN BZD CAD CDF CHF CLP CNY COP CRC CUP CVE CZK DJF DKK DOP DZD EGP ERN ETB EUR FJD \
FKP GBP GEL GHS GIP GMD GNF GTQ GYD HKD HNL HTG HUF IDR ILS INR IQD IRR ISK JMD JOD JPY KES KGS KHR \
KMF KPW KRW KWD KYD KZT LAK LBP LKR LRD LSL LYD MAD MDL MGA MKD MMK MNT MOP MRU MUR MVR MWK MXN MYR \
MZN NAD NGN NIO NOK NPR NZD OMR PAB PEN PGK PHP PKR PLN PYG QAR RON RSD RUB RWF SAR SBD SCR SDG SEK \
SGD SHP SLE SOS SRD SSP STN SVC SYP SZL THB TJS TMT TND TOP TRY TTD TWD TZS UAH UGX USD UYU UZS VES \
VND VUV WST XAF XCD XOF XPF YER ZAR ZMW ZWL";

fn sorted_list(mut values: Vec<&str>) -> Value {
    values.sort_unstable();
    values.dedup();
    Value::str(&values.join(SEPARATOR))
}

fn zones() -> impl Iterator<Item = (&'static str, &'static str)> {
    ZONES
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(_, zone)| parse_zone(zone).is_some())
}

/// `(key)` -> the sorted values of `currency`, `timeZone` or `unit`.
fn supported_values(_: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    Ok(match string_arg(args, 0).as_str() {
        "currency" => sorted_list(CURRENCIES.split_whitespace().collect()),
        "unit" => sorted_list(UNITS.to_vec()),
        "timeZone" => {
            let etc: Vec<String> = (1..=14)
                .map(|hours| format!("Etc/GMT-{hours}"))
                .chain((1..=12).map(|hours| format!("Etc/GMT+{hours}")))
                .collect();
            let etc = etc.iter().map(String::as_str);
            sorted_list(
                zones()
                    .map(|(_, zone)| zone)
                    .chain(["UTC"])
                    .chain(etc)
                    .collect(),
            )
        }
        _ => Value::Undefined,
    })
}

/// `(region)` -> `firstDay` (Monday = 1) and the weekend days, joined; empty when the
/// region has no week data.
fn locale_week(_: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let tag = format!("und-{}", string_arg(args, 0));
    let Ok(locale) = Locale::try_from_str(&tag) else {
        return Ok(Value::str(""));
    };
    let Ok(info) = WeekInformation::try_new(WeekPreferences::from(&locale)) else {
        return Ok(Value::str(""));
    };
    let mut weekend: Vec<i8> = info.weekend().map(|day| day as i8).collect();
    weekend.sort_unstable();
    let weekend: Vec<String> = weekend.iter().map(i8::to_string).collect();
    let first = (info.first_weekday as i8).to_string();
    Ok(Value::str(&format!(
        "{first}{SEPARATOR}{}",
        weekend.join(",")
    )))
}

/// `(region)` -> the sorted zones in common use there.
fn locale_zones(_: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let region = string_arg(args, 0);
    Ok(sorted_list(
        zones()
            .filter(|(code, _)| *code == region)
            .map(|(_, zone)| zone)
            .collect(),
    ))
}

pub(super) fn ops() -> Vec<(&'static str, u32, NativeFn)> {
    vec![
        ("__intl_supported_values", 1, Rc::new(supported_values)),
        ("__intl_locale_week", 1, Rc::new(locale_week)),
        ("__intl_locale_zones", 1, Rc::new(locale_zones)),
    ]
}

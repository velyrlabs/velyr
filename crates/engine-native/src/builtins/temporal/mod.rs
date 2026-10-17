//! Temporal (VELYR-173) on `temporal_rs`, reusing the tzdb and host zone of `Date`. The JS
//! classes live in the `prelude*.js` files and keep one canonical ISO string per object (in
//! a WeakMap); every operation goes through the single native op `__temporal_op(class,
//! method, ...string args)`, which parses the strings, calls `temporal_rs` and returns a
//! string. The prelude captures the op and deletes it from the global.
//! shortcut: `PlainYearMonth`/`PlainMonthDay` are iso8601 only (N48) and no type does calendar
//! arithmetic yet (N46); no `toLocaleString`; every call re-parses its receiver, so it is not
//! for hot loops.

mod duration;
mod instant;
mod now;
mod plain_date;
mod plain_date_time;
mod plain_month_day;
mod plain_time;
mod plain_year_month;
mod zoned;

use std::cmp::Ordering;
use std::rc::Rc;
use std::str::FromStr;

use temporal_rs::error::ErrorKind as TemporalKind;
use temporal_rs::options::{
    DifferenceSettings, Overflow, RoundingIncrement, RoundingOptions, ToStringRoundingOptions,
};
use temporal_rs::parsers::Precision;
use temporal_rs::{Calendar, Duration, TemporalError, TemporalResult, TimeZone};
use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::{Abrupt, ErrorKind, Interpreter, LazyInstall, NativeFn};

use crate::prelude::run_prelude;

const PRELUDES: &[(&str, &str)] = &[
    ("velyr:temporal", include_str!("prelude.js")),
    ("velyr:temporal_plain", include_str!("prelude_plain.js")),
    ("velyr:temporal_zoned", include_str!("prelude_zoned.js")),
    ("velyr:temporal_ym", include_str!("prelude_ym.js")),
    ("velyr:temporal_md", include_str!("prelude_md.js")),
    ("velyr:temporal_intl", include_str!("prelude_intl.js")),
];

type Res = TemporalResult<String>;
type Args<'a> = &'a [String];

fn arg(a: &[String], i: usize) -> &str {
    a.get(i).map_or("", String::as_str)
}

fn range(message: &'static str) -> TemporalError {
    TemporalError::range().with_message(message)
}

/// An optional enum-like argument: `""` is `undefined`.
fn opt<T: FromStr>(a: Args, i: usize, message: &'static str) -> TemporalResult<Option<T>> {
    match arg(a, i) {
        "" => Ok(None),
        text => text.parse().map(Some).map_err(|_| range(message)),
    }
}

/// An integer argument, saturated to `max` (the prelude rejects negatives and non-integers).
fn num<T: TryFrom<i64>>(a: Args, i: usize, max: i64) -> TemporalResult<T> {
    let n: i64 = arg(a, i).parse().map_err(|_| range("invalid number"))?;
    T::try_from(n.min(max)).map_err(|_| range("number out of range"))
}

fn overflow(a: Args, i: usize) -> TemporalResult<Option<Overflow>> {
    opt(a, i, "invalid overflow")
}

fn duration(a: Args, i: usize) -> TemporalResult<Duration> {
    Duration::from_str(arg(a, i))
}

fn time_zone(a: Args, i: usize) -> TemporalResult<TimeZone> {
    TimeZone::try_from_str(arg(a, i))
}

/// `largestUnit, smallestUnit, roundingMode, roundingIncrement` at `i..i + 4`.
fn difference(a: Args, i: usize) -> TemporalResult<DifferenceSettings> {
    let mut settings = DifferenceSettings::default();
    settings.largest_unit = opt(a, i, "invalid largestUnit")?;
    settings.smallest_unit = opt(a, i + 1, "invalid smallestUnit")?;
    settings.rounding_mode = opt(a, i + 2, "invalid roundingMode")?;
    settings.increment = increment(a, i + 3)?;
    Ok(settings)
}

fn rounding(a: Args, i: usize) -> TemporalResult<RoundingOptions> {
    let mut options = RoundingOptions::default();
    let settings = difference(a, i)?;
    options.largest_unit = settings.largest_unit.or(options.largest_unit);
    options.smallest_unit = settings.smallest_unit;
    options.rounding_mode = settings.rounding_mode;
    options.increment = settings.increment;
    Ok(options)
}

fn increment(a: Args, i: usize) -> TemporalResult<Option<RoundingIncrement>> {
    match arg(a, i) {
        "" => Ok(None),
        text => {
            let value: f64 = text
                .parse()
                .map_err(|_| range("invalid roundingIncrement"))?;
            RoundingIncrement::try_from(value).map(Some)
        }
    }
}

/// `fractionalSecondDigits ("auto", "0".."9"), smallestUnit, roundingMode` at `i..i + 3`.
fn text_options(a: Args, i: usize) -> TemporalResult<ToStringRoundingOptions> {
    let precision = match arg(a, i) {
        "" | "auto" => Precision::Auto,
        digits => Precision::Digit(digits.parse().map_err(|_| range("invalid digits"))?),
    };
    Ok(ToStringRoundingOptions {
        precision,
        smallest_unit: opt(a, i + 1, "invalid smallestUnit")?,
        rounding_mode: opt(a, i + 2, "invalid roundingMode")?,
    })
}

fn ordering(order: Ordering) -> Res {
    Ok((order as i8).to_string())
}

/// The calendar at `a[i]` (`""` is iso8601); a RangeError for an unsupported identifier.
fn calendar(a: Args, i: usize) -> TemporalResult<Calendar> {
    match arg(a, i) {
        "" => Ok(Calendar::ISO),
        id => parse_calendar(id),
    }
}

/// `islamic` only exists as an `Intl.DateTimeFormat` fallback, never as a Temporal calendar
/// (`temporal_rs` aliases it to `islamic-civil`).
fn parse_calendar(id: &str) -> TemporalResult<Calendar> {
    if id.eq_ignore_ascii_case("islamic") {
        return Err(range("the islamic calendar is not supported"));
    }
    Calendar::try_from_utf8(id.as_bytes())
}

fn ensure_iso(calendar: &Calendar) -> TemporalResult<()> {
    match calendar.identifier() {
        "iso8601" => Ok(()),
        _ => Err(range("only the iso8601 calendar is supported")),
    }
}

/// `year,month,day,dayOfWeek,dayOfYear,weekOfYear,yearOfWeek,daysInMonth,daysInYear,
/// monthsInYear,inLeapYear,monthCode` of any Temporal type with a date.
macro_rules! date_fields {
    ($d:expr) => {{
        let d = &$d;
        format!(
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            d.year(),
            d.month(),
            d.day(),
            d.day_of_week(),
            d.day_of_year(),
            d.week_of_year().unwrap_or_default(),
            d.year_of_week().unwrap_or_default(),
            d.days_in_month(),
            d.days_in_year(),
            d.months_in_year(),
            u8::from(d.in_leap_year()),
            d.month_code().as_str()
        )
    }};
}

/// `hour,minute,second,millisecond,microsecond,nanosecond`.
macro_rules! time_fields {
    ($t:expr) => {{
        let t = &$t;
        format!(
            "{},{},{},{},{},{}",
            t.hour(),
            t.minute(),
            t.second(),
            t.millisecond(),
            t.microsecond(),
            t.nanosecond()
        )
    }};
}
use {date_fields, time_fields};

/// Canonical id of a calendar (`"ISO8601"` -> `"iso8601"`) or a RangeError.
fn calendar_id(a: Args) -> Res {
    Ok(parse_calendar(arg(a, 0))?.identifier().to_owned())
}

/// The calendar annotation of a time-only string (`iso8601` without one).
fn time_string_calendar(text: &str) -> Res {
    match text
        .split_once("u-ca=")
        .and_then(|(_, rest)| rest.split(']').next())
    {
        Some(id) => calendar_id(&[id.to_owned()]),
        None => Ok("iso8601".to_owned()),
    }
}

/// Like `calendar_id`, but also accepts any ISO string and returns its calendar annotation
/// (`iso8601` without one), as `ToTemporalCalendarIdentifier` does.
fn calendar_of_text(a: Args) -> Res {
    let text = arg(a, 0);
    if let Ok(found) = calendar_id(a) {
        return Ok(found);
    }
    let bytes = text.as_bytes();
    let id = |c: &Calendar| c.identifier().to_owned();
    plain_date_time::parse(text)
        .map(|v| id(v.calendar()))
        .or_else(|_| plain_date::parse(text).map(|v| id(v.calendar())))
        .or_else(|_| {
            temporal_rs::PlainTime::from_utf8(bytes).and_then(|_| time_string_calendar(text))
        })
        .or_else(|_| temporal_rs::PlainYearMonth::from_utf8(bytes).map(|v| id(v.calendar())))
        .or_else(|_| temporal_rs::PlainMonthDay::from_utf8(bytes).map(|v| id(v.calendar())))
}

/// Canonical id of a time zone or offset string, or a RangeError.
fn timezone_id(a: Args) -> Res {
    time_zone(a, 0)?.identifier()
}

fn run(class: &str, method: &str, a: Args) -> Res {
    match class {
        "Calendar" => match method {
            "parse" => calendar_of_text(a),
            _ => calendar_id(a),
        },
        "TimeZone" => timezone_id(a),
        "PlainDate" => plain_date::run(method, a),
        "PlainTime" => plain_time::run(method, a),
        "PlainDateTime" => plain_date_time::run(method, a),
        "Instant" => instant::run(method, a),
        "ZonedDateTime" => zoned::run(method, a),
        "Duration" => duration::run(method, a),
        "PlainYearMonth" => plain_year_month::run(method, a),
        "PlainMonthDay" => plain_month_day::run(method, a),
        "Now" => now::run(method),
        _ => Err(TemporalError::general("unknown Temporal class")),
    }
}

fn op(vm: &mut Interpreter, _: &Value, args: &[Value]) -> Result<Value, Abrupt> {
    let strings: Vec<String> = args
        .iter()
        .map(|value| match value {
            Value::String(s) => s.to_string_lossy(),
            _ => String::new(),
        })
        .collect();
    let [class, method, rest @ ..] = strings.as_slice() else {
        return Err(vm.error(ErrorKind::Type, "invalid Temporal operation"));
    };
    run(class, method, rest)
        .map(|s| Value::str(&s))
        .map_err(|error| {
            let kind = match error.kind() {
                TemporalKind::Type => ErrorKind::Type,
                TemporalKind::Generic | TemporalKind::Assert => ErrorKind::Error,
                TemporalKind::Range | TemporalKind::Syntax => ErrorKind::Range,
            };
            vm.error(kind, error.into_message())
        })
}

/// Defines the lazy `Temporal` namespace on `vm`'s realm; the first operation on it defines
/// the native op and runs the preludes.
pub(crate) fn install(vm: &mut Interpreter) -> Result<(), String> {
    let init: LazyInstall =
        Rc::new(|vm| materialize(vm).map_err(|message| vm.error(ErrorKind::Error, &message)));
    vm.define_lazy_namespace("Temporal", init)
        .map_err(|_| "cannot define Temporal".to_owned())
}

fn materialize(vm: &mut Interpreter) -> Result<(), String> {
    let native: NativeFn = Rc::new(op);
    vm.define_global_function("__temporal_op", 2, native)
        .map_err(|_| "cannot define __temporal_op".to_owned())?;
    for &(name, source) in PRELUDES {
        run_prelude(vm, name, source)?;
    }
    Ok(())
}

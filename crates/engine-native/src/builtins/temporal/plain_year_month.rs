//! `Temporal.PlainYearMonth` ops; the state is `isoYear,isoMonth,referenceDay` (the reference
//! day is kept apart from the string form because dates at the range limits are not dates).

use temporal_rs::fields::CalendarFields;
use temporal_rs::options::DisplayCalendar;
use temporal_rs::{Calendar, PlainYearMonth, TemporalError, TemporalResult};

use super::{Args, Res, arg, difference, duration, ensure_iso, num, ordering, overflow, range};

fn state(value: &PlainYearMonth) -> String {
    format!(
        "{},{},{}",
        value.year(),
        value.month(),
        value.reference_day()
    )
}

fn parse_state(text: &str) -> TemporalResult<PlainYearMonth> {
    let parts: Vec<&str> = text.split(',').collect();
    let [year, month, day] = parts.as_slice() else {
        return Err(range("invalid PlainYearMonth state"));
    };
    let number = |s: &str| {
        s.parse::<i32>()
            .map_err(|_| range("invalid PlainYearMonth state"))
    };
    let narrow = |n: i32| u8::try_from(n).map_err(|_| range("invalid PlainYearMonth state"));
    PlainYearMonth::try_new_iso(
        number(year)?,
        narrow(number(month)?)?,
        Some(narrow(number(day)?)?),
    )
}

fn year_month(a: Args, i: usize) -> TemporalResult<PlainYearMonth> {
    parse_state(arg(a, i))
}

fn display(a: Args, i: usize) -> TemporalResult<DisplayCalendar> {
    Ok(match arg(a, i) {
        "always" => DisplayCalendar::Always,
        "never" => DisplayCalendar::Never,
        "critical" => DisplayCalendar::Critical,
        _ => DisplayCalendar::Auto,
    })
}

fn fields(value: &PlainYearMonth) -> String {
    format!(
        "{},{},{},{},{},{},{}",
        value.year(),
        value.month(),
        value.month_code().as_str(),
        value.days_in_year(),
        value.days_in_month(),
        value.months_in_year(),
        u8::from(value.in_leap_year())
    )
}

pub(super) fn run(method: &str, a: Args) -> Res {
    match method {
        "parse" => {
            let value = PlainYearMonth::from_utf8(arg(a, 0).as_bytes())?;
            ensure_iso(value.calendar())?;
            Ok(state(&value))
        }
        "create" => {
            let (year, month, day) = (
                num(a, 0, i32::MAX.into())?,
                num(a, 1, 255)?,
                num(a, 2, 255)?,
            );
            let overflow = overflow(a, 3)?.unwrap_or_default();
            PlainYearMonth::new_with_overflow(year, month, Some(day), Calendar::ISO, overflow)
                .map(|value| state(&value))
        }
        "fields" => Ok(fields(&year_month(a, 0)?)),
        "toString" => Ok(year_month(a, 0)?.to_ixdtf_string(display(a, 1)?)),
        "add" | "subtract" => {
            let (value, span) = (year_month(a, 0)?, duration(a, 1)?);
            let overflow = overflow(a, 2)?.unwrap_or_default();
            let result = if method == "add" {
                value.add(&span, overflow)
            } else {
                value.subtract(&span, overflow)
            };
            result.map(|value| state(&value))
        }
        "until" => year_month(a, 0)?
            .until(&year_month(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "since" => year_month(a, 0)?
            .since(&year_month(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "compare" => ordering(year_month(a, 0)?.compare_iso(&year_month(a, 1)?)),
        "toDate" => {
            let day = num(a, 1, 255)?;
            let date =
                year_month(a, 0)?.to_plain_date(Some(CalendarFields::new().with_day(day)))?;
            Ok(date.to_ixdtf_string(DisplayCalendar::Auto))
        }
        _ => Err(TemporalError::general("unknown PlainYearMonth op")),
    }
}

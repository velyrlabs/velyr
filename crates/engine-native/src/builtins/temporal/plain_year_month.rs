//! `Temporal.PlainYearMonth` ops; the state is `isoYear,isoMonth,referenceDay` (the reference
//! day is kept apart from the string form because dates at the range limits are not dates).

use temporal_rs::fields::CalendarFields;
use temporal_rs::options::DisplayCalendar;
use temporal_rs::partial::PartialYearMonth;
use temporal_rs::{Calendar, PlainYearMonth, TemporalError, TemporalResult};

use super::plain_date::{calendar_fields, era_fields};
use super::{
    Args, Res, arg, calendar, difference, duration, num, ordering, overflow, parse_calendar, range,
};

/// `year,month,day` of an ISO date printed as `YYYY-MM-DD[...]` (the year may be signed).
pub(super) fn iso_parts(ixdtf: &str) -> TemporalResult<(i32, u8, u8)> {
    let bad = || range("invalid ISO date");
    let date = ixdtf.split('[').next().unwrap_or_default();
    let (rest, day) = date.rsplit_once('-').ok_or_else(bad)?;
    let (year, month) = rest.rsplit_once('-').ok_or_else(bad)?;
    Ok((
        year.parse().map_err(|_| bad())?,
        month.parse().map_err(|_| bad())?,
        day.parse().map_err(|_| bad())?,
    ))
}

/// `isoYear,isoMonth,referenceDay`, plus `[u-ca=id]` for a calendar other than iso8601.
fn state(value: &PlainYearMonth) -> TemporalResult<String> {
    let id = value.calendar().identifier();
    if id == "iso8601" {
        return Ok(format!(
            "{},{},{}",
            value.year(),
            value.month(),
            value.reference_day()
        ));
    }
    let (year, month, day) = iso_parts(&value.to_ixdtf_string(DisplayCalendar::Always))?;
    Ok(format!("{year},{month},{day}[u-ca={id}]"))
}

fn parse_state(text: &str) -> TemporalResult<PlainYearMonth> {
    let (text, calendar) = match text.split_once("[u-ca=") {
        Some((body, id)) => (body, parse_calendar(id.trim_end_matches(']'))?),
        None => (text, Calendar::ISO),
    };
    let parts: Vec<&str> = text.split(',').collect();
    let [year, month, day] = parts.as_slice() else {
        return Err(range("invalid PlainYearMonth state"));
    };
    let number = |s: &str| {
        s.parse::<i32>()
            .map_err(|_| range("invalid PlainYearMonth state"))
    };
    let narrow = |n: i32| u8::try_from(n).map_err(|_| range("invalid PlainYearMonth state"));
    PlainYearMonth::try_new(
        number(year)?,
        narrow(number(month)?)?,
        Some(narrow(number(day)?)?),
        calendar,
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
        "{},{},{},{},{},{},{},{}",
        value.year(),
        value.month(),
        value.month_code().as_str(),
        value.days_in_year(),
        value.days_in_month(),
        value.months_in_year(),
        u8::from(value.in_leap_year()),
        era_fields(value.era(), value.era_year())
    )
}

pub(super) fn run(method: &str, a: Args) -> Res {
    match method {
        "parse" => state(&PlainYearMonth::from_utf8(arg(a, 0).as_bytes())?),
        "create" => {
            let (year, month, day) = (
                num(a, 0, i32::MAX.into())?,
                num(a, 1, 255)?,
                num(a, 2, 255)?,
            );
            let overflow = overflow(a, 3)?.unwrap_or_default();
            PlainYearMonth::new_with_overflow(year, month, Some(day), calendar(a, 4)?, overflow)
                .and_then(|value| state(&value))
        }
        "fromFields" => {
            let partial = PartialYearMonth {
                calendar_fields: calendar_fields(a, 1)?.into(),
                calendar: calendar(a, 0)?,
            };
            PlainYearMonth::from_partial(partial, overflow(a, 7)?).and_then(|v| state(&v))
        }
        "with" => year_month(a, 0)?
            .with(calendar_fields(a, 1)?.into(), overflow(a, 7)?)
            .and_then(|value| state(&value)),
        "equals" => Ok((year_month(a, 0)? == year_month(a, 1)?).to_string()),
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
            result.and_then(|value| state(&value))
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

//! `Temporal.PlainMonthDay` ops; the state is `isoMonth,isoDay,referenceYear` (the reference
//! year is kept apart from the string form because it is not always 1972).

use temporal_rs::options::{DisplayCalendar, Overflow};
use temporal_rs::partial::PartialDate;
use temporal_rs::{Calendar, PlainMonthDay, TemporalError, TemporalResult};

use super::plain_date::calendar_fields;
use super::plain_year_month::iso_parts;
use super::{Args, Res, arg, calendar, num, overflow, parse_calendar, range};

/// `isoMonth,isoDay,referenceYear`, plus `[u-ca=id]` for a calendar other than iso8601.
fn state(value: &PlainMonthDay) -> TemporalResult<String> {
    let id = value.calendar().identifier();
    if id == "iso8601" {
        return Ok(format!(
            "{},{},{}",
            value.month_code().to_month_integer(),
            value.day(),
            value.reference_year()
        ));
    }
    let (year, month, day) = iso_parts(&value.to_ixdtf_string(DisplayCalendar::Always))?;
    Ok(format!("{month},{day},{year}[u-ca={id}]"))
}

fn parse_state(text: &str) -> TemporalResult<PlainMonthDay> {
    let (text, calendar) = match text.split_once("[u-ca=") {
        Some((body, id)) => (body, parse_calendar(id.trim_end_matches(']'))?),
        None => (text, Calendar::ISO),
    };
    let parts: Vec<&str> = text.split(',').collect();
    let [month, day, year] = parts.as_slice() else {
        return Err(range("invalid PlainMonthDay state"));
    };
    let bad = || range("invalid PlainMonthDay state");
    PlainMonthDay::new_with_overflow(
        month.parse().map_err(|_| bad())?,
        day.parse().map_err(|_| bad())?,
        calendar,
        Overflow::Reject,
        Some(year.parse().map_err(|_| bad())?),
    )
}

fn month_day(a: Args, i: usize) -> TemporalResult<PlainMonthDay> {
    parse_state(arg(a, i))
}

pub(super) fn run(method: &str, a: Args) -> Res {
    match method {
        "parse" => state(&PlainMonthDay::from_utf8(arg(a, 0).as_bytes())?),
        "create" => PlainMonthDay::new_with_overflow(
            num(a, 0, 255)?,
            num(a, 1, 255)?,
            calendar(a, 4)?,
            overflow(a, 3)?.unwrap_or(Overflow::Reject),
            Some(num(a, 2, i32::MAX.into())?),
        )
        .and_then(|value| state(&value)),
        "fromFields" => {
            let partial = PartialDate {
                calendar_fields: calendar_fields(a, 1)?,
                calendar: calendar(a, 0)?,
            };
            PlainMonthDay::from_partial(partial, overflow(a, 7)?).and_then(|v| state(&v))
        }
        "with" => month_day(a, 0)?
            .with(calendar_fields(a, 1)?, overflow(a, 7)?)
            .and_then(|value| state(&value)),
        "fields" => {
            let value = month_day(a, 0)?;
            Ok(format!("{},{}", value.month_code().as_str(), value.day()))
        }
        "toString" => {
            let display = match arg(a, 1) {
                "always" => DisplayCalendar::Always,
                "never" => DisplayCalendar::Never,
                "critical" => DisplayCalendar::Critical,
                _ => DisplayCalendar::Auto,
            };
            Ok(month_day(a, 0)?.to_ixdtf_string(display))
        }
        "equals" => Ok((month_day(a, 0)? == month_day(a, 1)?).to_string()),
        "toDate" => {
            let date = month_day(a, 0)?.to_plain_date(Some(calendar_fields(a, 1)?))?;
            Ok(date.to_ixdtf_string(DisplayCalendar::Auto))
        }
        _ => Err(TemporalError::general("unknown PlainMonthDay op")),
    }
}

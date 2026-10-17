//! `Temporal.PlainDate` ops; the state is the ISO date string.

use temporal_rs::fields::CalendarFields;
use temporal_rs::options::DisplayCalendar;
use temporal_rs::partial::PartialDate;
use temporal_rs::{MonthCode, PlainDate, TemporalResult};

use super::{
    Args, Res, arg, calendar, date_fields, difference, duration, num, ordering, overflow, time_zone,
};

pub(super) fn parse(text: &str) -> temporal_rs::TemporalResult<PlainDate> {
    PlainDate::from_utf8(text.as_bytes())
}

fn date(a: Args, i: usize) -> temporal_rs::TemporalResult<PlainDate> {
    parse(arg(a, i))
}

/// `"era,eraYear"` (`""` for a calendar without eras).
pub(super) fn era_fields(era: Option<impl ToString>, era_year: Option<i32>) -> String {
    let era = era.map(|era| era.to_string()).unwrap_or_default();
    let year = era_year.map(|year| year.to_string()).unwrap_or_default();
    format!("{era},{year}")
}

fn number<T: TryFrom<i64>>(a: Args, i: usize) -> TemporalResult<Option<T>> {
    match arg(a, i) {
        "" => Ok(None),
        text => {
            let value: f64 = text.parse().map_err(|_| super::range("invalid number"))?;
            if value.abs() > f64::from(i32::MAX) {
                return Err(super::range("number out of range"));
            }
            T::try_from(value as i64)
                .map(Some)
                .map_err(|_| super::range("number out of range"))
        }
    }
}

/// `year, month, monthCode, day, era, eraYear` at `i..i + 6` (`""` is `undefined`).
pub(super) fn calendar_fields(a: Args, i: usize) -> TemporalResult<CalendarFields> {
    let month_code = match arg(a, i + 2) {
        "" => None,
        code => Some(code.parse::<MonthCode>()?),
    };
    let era = match arg(a, i + 4) {
        "" => None,
        era => Some(era.parse().map_err(|_| super::range("invalid era"))?),
    };
    Ok(CalendarFields::new()
        .with_optional_year(number(a, i)?)
        .with_optional_month(number(a, i + 1)?)
        .with_optional_month_code(month_code)
        .with_optional_day(number(a, i + 3)?)
        .with_era(era)
        .with_era_year(number(a, i + 5)?))
}

pub(super) fn run(method: &str, a: Args) -> Res {
    let text = |d: PlainDate| d.to_ixdtf_string(DisplayCalendar::Auto);
    match method {
        "parse" => date(a, 0).map(text),
        "create" => {
            let (year, month, day) = (
                num(a, 0, i32::MAX.into())?,
                num(a, 1, 255)?,
                num(a, 2, 255)?,
            );
            let overflow = overflow(a, 3)?.unwrap_or_default();
            PlainDate::new_with_overflow(year, month, day, calendar(a, 4)?, overflow).map(text)
        }
        "withCalendar" => Ok(text(date(a, 0)?.with_calendar(calendar(a, 1)?))),
        "fields" => {
            let value = date(a, 0)?;
            Ok(format!(
                "{},{}",
                date_fields!(value),
                era_fields(value.era(), value.era_year())
            ))
        }
        "fromFields" => {
            let partial = PartialDate {
                calendar_fields: calendar_fields(a, 1)?,
                calendar: calendar(a, 0)?,
            };
            PlainDate::from_partial(partial, overflow(a, 7)?).map(text)
        }
        "withFields" => date(a, 0)?
            .with(calendar_fields(a, 1)?, overflow(a, 7)?)
            .map(text),
        "add" => date(a, 0)?.add(&duration(a, 1)?, overflow(a, 2)?).map(text),
        "subtract" => date(a, 0)?
            .subtract(&duration(a, 1)?, overflow(a, 2)?)
            .map(text),
        "until" => date(a, 0)?
            .until(&date(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "since" => date(a, 0)?
            .since(&date(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "equals" => Ok((date(a, 0)? == date(a, 1)?).to_string()),
        "compare" => ordering(date(a, 0)?.compare_iso(&date(a, 1)?)),
        "toZoned" => {
            let time = match arg(a, 2) {
                "" => None,
                t => Some(super::plain_time::parse(t)?),
            };
            let zoned = date(a, 0)?.to_zoned_date_time(time_zone(a, 1)?, time)?;
            Ok(zoned.to_string())
        }
        _ => Err(temporal_rs::TemporalError::general("unknown PlainDate op")),
    }
}

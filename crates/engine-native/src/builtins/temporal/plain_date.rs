//! `Temporal.PlainDate` ops; the state is the ISO date string.

use temporal_rs::PlainDate;
use temporal_rs::options::DisplayCalendar;

use super::{
    Args, Res, arg, calendar, date_fields, difference, duration, num, ordering, overflow, time_zone,
};

pub(super) fn parse(text: &str) -> temporal_rs::TemporalResult<PlainDate> {
    PlainDate::from_utf8(text.as_bytes())
}

fn date(a: Args, i: usize) -> temporal_rs::TemporalResult<PlainDate> {
    parse(arg(a, i))
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
        "fields" => Ok(date_fields!(date(a, 0)?)),
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

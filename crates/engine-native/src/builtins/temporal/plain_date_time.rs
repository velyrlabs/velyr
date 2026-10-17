//! `Temporal.PlainDateTime` ops; the state is the ISO date-time string.

use temporal_rs::options::DisplayCalendar;
use temporal_rs::{PlainDateTime, TemporalError, TemporalResult};

use super::{
    Args, Res, arg, calendar, date_fields, difference, duration, num, opt, ordering, overflow,
    rounding, text_options, time_fields, time_zone,
};

pub(super) fn parse(text: &str) -> TemporalResult<PlainDateTime> {
    PlainDateTime::from_utf8(text.as_bytes())
}

fn datetime(a: Args, i: usize) -> TemporalResult<PlainDateTime> {
    parse(arg(a, i))
}

pub(super) fn run(method: &str, a: Args) -> Res {
    let text = |d: PlainDateTime| d.to_ixdtf_string(Default::default(), DisplayCalendar::Auto);
    match method {
        "parse" => datetime(a, 0).and_then(text),
        "create" => {
            let date = (
                num(a, 0, i32::MAX.into())?,
                num(a, 1, 255)?,
                num(a, 2, 255)?,
            );
            let time = (num(a, 3, 255)?, num(a, 4, 255)?, num(a, 5, 255)?);
            let sub = (num(a, 6, 65_535)?, num(a, 7, 65_535)?, num(a, 8, 65_535)?);
            let overflow = overflow(a, 9)?.unwrap_or_default();
            let (y, mo, d) = date;
            let (h, mi, s) = time;
            let (ms, us, ns) = sub;
            PlainDateTime::new_with_overflow(
                y,
                mo,
                d,
                h,
                mi,
                s,
                ms,
                us,
                ns,
                calendar(a, 10)?,
                overflow,
            )
            .and_then(text)
        }
        "withCalendar" => Ok(text(datetime(a, 0)?.with_calendar(calendar(a, 1)?))?),
        "fields" => {
            let value = datetime(a, 0)?;
            Ok(format!("{},{}", date_fields!(value), time_fields!(value)))
        }
        "add" => datetime(a, 0)?
            .add(&duration(a, 1)?, overflow(a, 2)?)
            .and_then(text),
        "subtract" => datetime(a, 0)?
            .subtract(&duration(a, 1)?, overflow(a, 2)?)
            .and_then(text),
        "until" => datetime(a, 0)?
            .until(&datetime(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "since" => datetime(a, 0)?
            .since(&datetime(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "round" => datetime(a, 0)?.round(rounding(a, 1)?).and_then(text),
        "compare" => ordering(datetime(a, 0)?.compare_iso(&datetime(a, 1)?)),
        "toString" => datetime(a, 0)?.to_ixdtf_string(text_options(a, 1)?, DisplayCalendar::Auto),
        "toZoned" => {
            let disambiguation = opt(a, 2, "invalid disambiguation")?.unwrap_or_default();
            let zoned = datetime(a, 0)?.to_zoned_date_time(time_zone(a, 1)?, disambiguation)?;
            Ok(zoned.to_string())
        }
        _ => Err(TemporalError::general("unknown PlainDateTime op")),
    }
}

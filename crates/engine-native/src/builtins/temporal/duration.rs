//! `Temporal.Duration` ops. Duration results travel as ten comma-separated integers
//! (`years,...,nanoseconds`), because an ISO 8601 string folds sub-second fields above 999.
//! Duration arguments accept either that form or an ISO string.

use std::str::FromStr;

use temporal_rs::options::RelativeTo;
use temporal_rs::{Duration, TemporalError, TemporalResult};

use super::{Args, Res, arg, opt, ordering, range, rounding, text_options};

fn relative_to(a: Args, i: usize) -> TemporalResult<Option<RelativeTo>> {
    match arg(a, i) {
        "" => Ok(None),
        text => RelativeTo::try_from_str(text).map(Some),
    }
}

fn int<T: std::str::FromStr>(a: Args, i: usize) -> TemporalResult<T> {
    arg(a, i)
        .parse()
        .map_err(|_| range("invalid duration field"))
}

/// The exact field form of `d`: `years,months,weeks,days,hours,minutes,seconds,ms,us,ns`.
pub(super) fn exact(d: &Duration) -> String {
    format!(
        "{},{},{},{},{},{},{},{},{},{}",
        d.years(),
        d.months(),
        d.weeks(),
        d.days(),
        d.hours(),
        d.minutes(),
        d.seconds(),
        d.milliseconds(),
        d.microseconds(),
        d.nanoseconds()
    )
}

/// A duration argument in the exact field form or as an ISO string.
fn duration(a: Args, i: usize) -> TemporalResult<Duration> {
    let text = arg(a, i);
    let unsigned = text.trim_start_matches(['+', '-']);
    if !text.contains(',') || unsigned.starts_with(['P', 'p']) {
        return Duration::from_str(text);
    }
    let field = |n: usize| text.split(',').nth(n).unwrap_or_default();
    let parse = |n: usize| {
        field(n)
            .parse::<i64>()
            .map_err(|_| range("invalid duration field"))
    };
    let wide = |n: usize| {
        field(n)
            .parse::<i128>()
            .map_err(|_| range("invalid duration field"))
    };
    Duration::new(
        parse(0)?,
        parse(1)?,
        parse(2)?,
        parse(3)?,
        parse(4)?,
        parse(5)?,
        parse(6)?,
        parse(7)?,
        wide(8)?,
        wide(9)?,
    )
}

pub(super) fn run(method: &str, a: Args) -> Res {
    match method {
        "parse" => Ok(duration(a, 0)?.to_string()),
        "parseExact" => Ok(exact(&duration(a, 0)?)),
        "iso" => Ok(duration(a, 0)?.to_string()),
        "create" | "createExact" => {
            let (y, mo, w, d) = (int(a, 0)?, int(a, 1)?, int(a, 2)?, int(a, 3)?);
            let (h, mi, s, ms) = (int(a, 4)?, int(a, 5)?, int(a, 6)?, int(a, 7)?);
            let created = Duration::new(y, mo, w, d, h, mi, s, ms, int(a, 8)?, int(a, 9)?)?;
            Ok(if method == "create" {
                created.to_string()
            } else {
                exact(&created)
            })
        }
        "negated" => Ok(exact(&duration(a, 0)?.negated())),
        "abs" => Ok(exact(&duration(a, 0)?.abs())),
        "add" => Ok(exact(&duration(a, 0)?.add(&duration(a, 1)?)?)),
        "subtract" => Ok(exact(&duration(a, 0)?.subtract(&duration(a, 1)?)?)),
        "round" => {
            let rounded = duration(a, 0)?.round(rounding(a, 1)?, relative_to(a, 5)?)?;
            Ok(exact(&rounded))
        }
        "total" => {
            let unit = opt(a, 1, "invalid unit")?.ok_or_else(|| range("unit is required"))?;
            Ok(duration(a, 0)?
                .total(unit, relative_to(a, 2)?)?
                .as_inner()
                .to_string())
        }
        "compare" => ordering(duration(a, 0)?.compare(&duration(a, 1)?, relative_to(a, 2)?)?),
        "toString" => duration(a, 0)?.as_temporal_string(text_options(a, 1)?),
        _ => Err(TemporalError::general("unknown Duration op")),
    }
}

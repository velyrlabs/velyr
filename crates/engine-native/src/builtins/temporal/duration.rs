//! `Temporal.Duration` ops; the state is the ISO 8601 duration string.

use temporal_rs::options::RelativeTo;
use temporal_rs::{Duration, TemporalError, TemporalResult};

use super::{Args, Res, arg, duration, opt, ordering, range, rounding, text_options};

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

pub(super) fn run(method: &str, a: Args) -> Res {
    match method {
        "parse" => Ok(duration(a, 0)?.to_string()),
        "create" => {
            let (y, mo, w, d) = (int(a, 0)?, int(a, 1)?, int(a, 2)?, int(a, 3)?);
            let (h, mi, s, ms) = (int(a, 4)?, int(a, 5)?, int(a, 6)?, int(a, 7)?);
            let created = Duration::new(y, mo, w, d, h, mi, s, ms, int(a, 8)?, int(a, 9)?)?;
            Ok(created.to_string())
        }
        "fields" => {
            let d = duration(a, 0)?;
            Ok(format!(
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
            ))
        }
        "negated" => Ok(duration(a, 0)?.negated().to_string()),
        "abs" => Ok(duration(a, 0)?.abs().to_string()),
        "add" => Ok(duration(a, 0)?.add(&duration(a, 1)?)?.to_string()),
        "subtract" => Ok(duration(a, 0)?.subtract(&duration(a, 1)?)?.to_string()),
        "round" => {
            let rounded = duration(a, 0)?.round(rounding(a, 1)?, relative_to(a, 5)?)?;
            Ok(rounded.to_string())
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

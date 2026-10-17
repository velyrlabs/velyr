//! `Temporal.Instant` ops; the state is the UTC ISO string.

use temporal_rs::{Instant, TemporalError, TemporalResult};

use super::{
    Args, Res, arg, difference, duration, ordering, range, rounding, text_options, time_zone,
};

fn parse(text: &str) -> TemporalResult<Instant> {
    Instant::from_utf8(text.as_bytes())
}

fn instant(a: Args, i: usize) -> TemporalResult<Instant> {
    parse(arg(a, i))
}

fn text(value: &Instant) -> Res {
    value.to_ixdtf_string(None, Default::default())
}

pub(super) fn run(method: &str, a: Args) -> Res {
    match method {
        "parse" => text(&instant(a, 0)?),
        "create" => {
            let ns = arg(a, 0)
                .parse()
                .map_err(|_| range("invalid epoch nanoseconds"))?;
            text(&Instant::try_new(ns)?)
        }
        "epochNanoseconds" => Ok(instant(a, 0)?.as_i128().to_string()),
        "epochMilliseconds" => Ok(instant(a, 0)?.epoch_milliseconds().to_string()),
        "add" => text(&instant(a, 0)?.add(&duration(a, 1)?)?),
        "subtract" => text(&instant(a, 0)?.subtract(&duration(a, 1)?)?),
        "until" => instant(a, 0)?
            .until(&instant(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "since" => instant(a, 0)?
            .since(&instant(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "round" => text(&instant(a, 0)?.round(rounding(a, 1)?)?),
        "compare" => ordering(instant(a, 0)?.as_i128().cmp(&instant(a, 1)?.as_i128())),
        "toString" => {
            let zone = match arg(a, 1) {
                "" => None,
                id => Some(temporal_rs::TimeZone::try_from_str(id)?),
            };
            instant(a, 0)?.to_ixdtf_string(zone, text_options(a, 2)?)
        }
        "toZoned" => Ok(instant(a, 0)?
            .to_zoned_date_time_iso(time_zone(a, 1)?)?
            .to_string()),
        _ => Err(TemporalError::general("unknown Instant op")),
    }
}

//! `Temporal.PlainTime` ops; the state is the ISO time string.

use temporal_rs::{PlainTime, TemporalError, TemporalResult};

use super::{
    Args, Res, arg, difference, duration, num, ordering, overflow, rounding, text_options,
    time_fields,
};

pub(super) fn parse(text: &str) -> TemporalResult<PlainTime> {
    PlainTime::from_utf8(text.as_bytes())
}

fn time(a: Args, i: usize) -> TemporalResult<PlainTime> {
    parse(arg(a, i))
}

fn key(t: &PlainTime) -> (u8, u8, u8, u16, u16, u16) {
    (
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond(),
        t.microsecond(),
        t.nanosecond(),
    )
}

pub(super) fn run(method: &str, a: Args) -> Res {
    let text = |t: PlainTime| t.to_ixdtf_string(Default::default());
    match method {
        "parse" => time(a, 0).and_then(text),
        "create" => {
            let (h, mi, s) = (num(a, 0, 255)?, num(a, 1, 255)?, num(a, 2, 255)?);
            let (ms, us, ns) = (num(a, 3, 65_535)?, num(a, 4, 65_535)?, num(a, 5, 65_535)?);
            let overflow = overflow(a, 6)?.unwrap_or_default();
            PlainTime::new_with_overflow(h, mi, s, ms, us, ns, overflow).and_then(text)
        }
        "fields" => Ok(time_fields!(time(a, 0)?)),
        "add" => time(a, 0)?.add(&duration(a, 1)?).and_then(text),
        "subtract" => time(a, 0)?.subtract(&duration(a, 1)?).and_then(text),
        "until" => time(a, 0)?
            .until(&time(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "since" => time(a, 0)?
            .since(&time(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "round" => time(a, 0)?.round(rounding(a, 1)?).and_then(text),
        "compare" => ordering(key(&time(a, 0)?).cmp(&key(&time(a, 1)?))),
        "toString" => time(a, 0)?.to_ixdtf_string(text_options(a, 1)?),
        _ => Err(TemporalError::general("unknown PlainTime op")),
    }
}

//! `Temporal.ZonedDateTime` ops; the state is the IXDTF string with offset and `[zone]`.

use temporal_rs::options::{Disambiguation, OffsetDisambiguation};
use temporal_rs::{Instant, TemporalError, TemporalResult, ZonedDateTime};

use super::{
    Args, Res, arg, date_fields, difference, duration, ensure_iso, opt, ordering, overflow, range,
    rounding, text_options, time_fields, time_zone,
};

fn parse(
    text: &str,
    disambiguation: Disambiguation,
    offset: OffsetDisambiguation,
) -> TemporalResult<ZonedDateTime> {
    let value = ZonedDateTime::from_utf8(text.as_bytes(), disambiguation, offset)?;
    ensure_iso(value.calendar())?;
    Ok(value)
}

/// A canonical string: its offset is trusted (`use`), since the offset is part of the
/// state. `reject` would re-validate it against the tzdb for every call.
fn zoned(a: Args, i: usize) -> TemporalResult<ZonedDateTime> {
    parse(
        arg(a, i),
        Disambiguation::Compatible,
        OffsetDisambiguation::Use,
    )
}

pub(super) fn run(method: &str, a: Args) -> Res {
    let text = |z: ZonedDateTime| z.to_string();
    match method {
        // text, disambiguation, offset option
        "parse" => {
            let disambiguation = opt(a, 1, "invalid disambiguation")?.unwrap_or_default();
            let offset = opt(a, 2, "invalid offset")?.unwrap_or(OffsetDisambiguation::Reject);
            parse(arg(a, 0), disambiguation, offset).map(text)
        }
        "create" => {
            let ns = arg(a, 0)
                .parse()
                .map_err(|_| range("invalid epoch nanoseconds"))?;
            ZonedDateTime::try_new_iso(ns, time_zone(a, 1)?).map(text)
        }
        "fields" => {
            let z = zoned(a, 0)?;
            Ok(format!(
                "{},{},{},{},{},{},{}",
                date_fields!(z),
                time_fields!(z),
                z.offset(),
                z.offset_nanoseconds(),
                z.epoch_nanoseconds().as_i128(),
                z.time_zone().identifier()?,
                z.hours_in_day()?
            ))
        }
        "add" => zoned(a, 0)?
            .add(&duration(a, 1)?, overflow(a, 2)?)
            .map(text),
        "subtract" => zoned(a, 0)?
            .subtract(&duration(a, 1)?, overflow(a, 2)?)
            .map(text),
        "until" => Ok(zoned(a, 0)?
            .until(&zoned(a, 1)?, difference(a, 2)?)?
            .to_string()),
        "since" => Ok(zoned(a, 0)?
            .since(&zoned(a, 1)?, difference(a, 2)?)?
            .to_string()),
        "round" => zoned(a, 0)?.round(rounding(a, 1)?).map(text),
        "startOfDay" => zoned(a, 0)?.start_of_day().map(text),
        "withTimeZone" => zoned(a, 0)?.with_timezone(time_zone(a, 1)?).map(text),
        "compare" => ordering(zoned(a, 0)?.compare_instant(&zoned(a, 1)?)),
        "equals" => Ok(zoned(a, 0)?.equals(&zoned(a, 1)?)?.to_string()),
        "toInstant" => Instant::try_new(zoned(a, 0)?.epoch_nanoseconds().as_i128())?
            .to_ixdtf_string(None, Default::default()),
        // text, offset, timeZoneName, calendarName, digits, smallestUnit, roundingMode
        "toString" => {
            let offset = opt(a, 1, "invalid offset")?.unwrap_or_default();
            let zone_name = opt(a, 2, "invalid timeZoneName")?.unwrap_or_default();
            let calendar = opt(a, 3, "invalid calendarName")?.unwrap_or_default();
            zoned(a, 0)?.to_ixdtf_string(offset, zone_name, calendar, text_options(a, 4)?)
        }
        _ => Err(TemporalError::general("unknown ZonedDateTime op")),
    }
}

//! `Temporal.ZonedDateTime` ops; the state is the IXDTF string with offset and `[zone]`.

use temporal_rs::fields::ZonedDateTimeFields;
use temporal_rs::options::{Disambiguation, OffsetDisambiguation};
use temporal_rs::partial::PartialZonedDateTime;
use temporal_rs::provider::TransitionDirection;
use temporal_rs::{
    Instant, PlainTime, TemporalError, TemporalResult, TimeZone, UtcOffset, ZonedDateTime,
};

use super::{
    Args, Res, arg, calendar, date_fields, difference, duration, opt, ordering, overflow, range,
    rounding, text_options, time_fields, time_zone,
};

fn parse(
    text: &str,
    disambiguation: Disambiguation,
    offset: OffsetDisambiguation,
) -> TemporalResult<ZonedDateTime> {
    ZonedDateTime::from_utf8(text.as_bytes(), disambiguation, offset)
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

/// `offset, year, month, monthCode, day, era, eraYear, hour..nanosecond` at `i..i + 13`.
fn zoned_fields(a: Args, i: usize) -> TemporalResult<ZonedDateTimeFields> {
    let offset = match arg(a, i) {
        "" => None,
        text => Some(text.parse::<UtcOffset>()?),
    };
    Ok(ZonedDateTimeFields {
        calendar_fields: super::plain_date::calendar_fields(a, i + 1)?,
        time: super::plain_date_time::time_parts(a, i + 7)?,
        offset,
    })
}

/// `disambiguation, offset, overflow` at `i..i + 3`.
fn choices(
    a: Args,
    i: usize,
) -> TemporalResult<(
    Option<Disambiguation>,
    Option<OffsetDisambiguation>,
    Option<temporal_rs::options::Overflow>,
)> {
    Ok((
        opt(a, i, "invalid disambiguation")?,
        opt(a, i + 1, "invalid offset")?,
        overflow(a, i + 2)?,
    ))
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
            ZonedDateTime::try_new(ns, time_zone(a, 1)?, calendar(a, 2)?).map(text)
        }
        "withCalendar" => Ok(text(zoned(a, 0)?.with_calendar(calendar(a, 1)?))),
        // calendar, time zone, then `zoned_fields` and `choices`
        "fromFields" => {
            let (disambiguation, offset, overflow) = choices(a, 15)?;
            let partial = PartialZonedDateTime {
                fields: zoned_fields(a, 2)?,
                timezone: Some(time_zone(a, 1)?),
                calendar: calendar(a, 0)?,
            };
            ZonedDateTime::from_partial(partial, overflow, disambiguation, offset).map(text)
        }
        // text, then `zoned_fields` and `choices`
        "withFields" => {
            let (disambiguation, offset, overflow) = choices(a, 14)?;
            zoned(a, 0)?
                .with(zoned_fields(a, 1)?, disambiguation, offset, overflow)
                .map(text)
        }
        "fields" => {
            let z = zoned(a, 0)?;
            Ok(format!(
                "{},{},{},{},{},{},{},{}",
                date_fields!(z),
                time_fields!(z),
                z.offset(),
                z.offset_nanoseconds(),
                z.epoch_nanoseconds().as_i128(),
                z.time_zone().identifier()?,
                z.hours_in_day()?,
                super::plain_date::era_fields(z.era(), z.era_year())
            ))
        }
        "add" => zoned(a, 0)?
            .add(&duration(a, 1)?, overflow(a, 2)?)
            .map(text),
        "subtract" => zoned(a, 0)?
            .subtract(&duration(a, 1)?, overflow(a, 2)?)
            .map(text),
        "until" => zoned(a, 0)?
            .until(&zoned(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "since" => zoned(a, 0)?
            .since(&zoned(a, 1)?, difference(a, 2)?)
            .map(|d| super::duration::exact(&d)),
        "round" => zoned(a, 0)?.round(rounding(a, 1)?).map(text),
        "startOfDay" => zoned(a, 0)?.start_of_day().map(text),
        // time text, or "" for the start of the day
        "withPlainTime" => {
            let time = match arg(a, 1) {
                "" => None,
                text => Some(PlainTime::from_utf8(text.as_bytes())?),
            };
            zoned(a, 0)?.with_plain_time(time).map(text)
        }
        // "next" | "previous"; "" means no transition (`null`)
        "transition" => {
            let direction: TransitionDirection =
                opt(a, 1, "invalid direction")?.ok_or_else(|| range("direction is required"))?;
            let found = zoned(a, 0)?.get_time_zone_transition(direction)?;
            Ok(found.map(text).unwrap_or_default())
        }
        // an identifier only (ISO date-time strings are not time zone identifiers)
        "zoneId" => TimeZone::try_from_identifier_str(arg(a, 0))?.identifier(),
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

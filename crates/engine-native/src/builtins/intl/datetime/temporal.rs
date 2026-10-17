//! Temporal plain types are formatted as the wall clock they carry: the prelude passes
//! the fields as milliseconds since 1970-01-01T00:00 and no time zone applies.

use icu_calendar::Iso;
use icu_calendar::types::RataDie;
use icu_datetime::input::{Date, DateTime, Time, TimeZone, UtcOffset};

use super::Instant;

const MS_PER_DAY: i64 = 86_400_000;
/// Rata die of 1970-01-01.
const EPOCH_RATA_DIE: i64 = 719_163;

/// The wall clock `local` (ms) with the zone data of the instant it came from, if any.
/// `Date::try_new_iso` stops at year 9999; the rata die constructor covers the whole
/// ECMAScript time range and the larger Temporal one.
pub(super) fn at_local(
    local: i64,
    zone: TimeZone,
    offset: Option<UtcOffset>,
    zoned: bool,
) -> Option<Instant> {
    let date = Date::from_rata_die(
        RataDie::new(local.div_euclid(MS_PER_DAY) + EPOCH_RATA_DIE),
        Iso,
    );
    let of_day = local.rem_euclid(MS_PER_DAY) as u32;
    let (seconds_of_day, millis) = (of_day / 1000, of_day % 1000);
    let time = Time::try_new(
        (seconds_of_day / 3600) as u8,
        (seconds_of_day / 60 % 60) as u8,
        (seconds_of_day % 60) as u8,
        millis * 1_000_000,
    )
    .ok()?;
    Some(Instant {
        wall: DateTime { date, time },
        zone,
        offset,
        zoned,
    })
}

pub(super) fn wall_instant(local: f64) -> Option<Instant> {
    local
        .is_finite()
        .then(|| at_local(local as i64, TimeZone::UNKNOWN, None, false))
        .flatten()
}

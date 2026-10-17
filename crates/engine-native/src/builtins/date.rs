//! The local time zone of `Date` (VELYR-114): `TZ` when it names an IANA zone, else the
//! host zone (the `/etc/localtime` link, else `iana-time-zone`), else UTC; offsets come from the tzdb compiled into
//! `temporal_rs`.

use std::rc::Rc;

use temporal_rs::{TimeZone, ZonedDateTime};
use velyr_engine_vm::interpreter::{Interpreter, LocalTimeZone};

const MS_PER_DAY: f64 = 86_400_000.0;
const MAX_TIME: f64 = 8.64e15;

/// An IANA zone or offset (`UTC`, `America/Sao_Paulo`, `+05:30`).
pub(crate) fn parse_zone(id: &str) -> Option<TimeZone> {
    TimeZone::try_from_identifier_str(id).ok()
}

/// Offset of `zone` at the instant `t` (ms), east positive; 0 outside the Temporal range.
pub(crate) fn offset_ms(zone: TimeZone, t: f64) -> f64 {
    if !t.is_finite() {
        return 0.0;
    }
    let nanos = t as i128 * 1_000_000;
    ZonedDateTime::try_new_iso(nanos, zone).map_or(0.0, |zdt| zdt.offset_nanoseconds() as f64 / 1e6)
}

/// Offset to subtract from the wall-clock time `local` of `zone`: of the offsets a day
/// before and after, the one that maps back to `local`. Overlaps take the earlier instant,
/// gaps the offset before the transition (Temporal's `compatible`).
pub(crate) fn offset_from_local(zone: TimeZone, local: f64) -> f64 {
    // Sample inside the representable range so wall-clock times just past a limit still
    // get the zone's offset there instead of 0.
    let sample = |t: f64| offset_ms(zone, t.clamp(-MAX_TIME, MAX_TIME));
    let before = sample(local - MS_PER_DAY);
    let after = sample(local + MS_PER_DAY);
    let fits = |offset: f64| offset_ms(zone, local - offset) == offset;
    match (fits(before), fits(after)) {
        (true, true) => before.max(after),
        (false, true) => after,
        _ => before,
    }
}

pub(crate) fn host_zone() -> TimeZone {
    let from_env = std::env::var("TZ").ok().and_then(|id| parse_zone(&id));
    let from_host = || {
        localtime_link()
            .or_else(|| iana_time_zone::get_timezone().ok())
            .and_then(|id| parse_zone(&id))
    };
    from_env.or_else(from_host).unwrap_or_else(TimeZone::utc)
}

/// The zone id `/etc/localtime` links to (`.../zoneinfo/America/Sao_Paulo`). Tried first
/// because `iana_time_zone` on macOS goes through CoreFoundation, which keeps ~36 MB of
/// ICU time zone data allocated for the life of the process.
fn localtime_link() -> Option<String> {
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let (_, id) = target.to_str()?.split_once("zoneinfo/")?;
    Some(id.to_owned())
}

struct HostZone(TimeZone);

impl LocalTimeZone for HostZone {
    fn offset_from_utc(&self, t: f64) -> f64 {
        offset_ms(self.0, t)
    }

    fn offset_from_local(&self, t: f64) -> f64 {
        offset_from_local(self.0, t)
    }

    /// The CLDR long name V8 prints, e.g. `Brasilia Standard Time`.
    fn name(&self, t: f64) -> Option<String> {
        let id = self.0.identifier().ok()?;
        crate::intl::zone_long_name(&id, t)
    }
}

pub(crate) fn install(vm: &mut Interpreter) {
    vm.set_local_time_zone(Rc::new(HostZone(host_zone())));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_offsets_follow_transitions() {
        let new_york = parse_zone("America/New_York").expect("zone");
        // 2021-03-14 02:30 does not exist (gap): it resolves to 03:30 EDT (07:30Z).
        let gap = 1_615_689_000_000.0;
        assert_eq!(gap - offset_from_local(new_york, gap), 1_615_707_000_000.0);
        // 2021-11-07 01:30 happens twice: the earlier instant is 05:30Z (EDT).
        let overlap = 1_636_248_600_000.0;
        assert_eq!(
            overlap - offset_from_local(new_york, overlap),
            1_636_263_000_000.0
        );
        assert_eq!(offset_ms(new_york, 0.0), -5.0 * 3_600_000.0);
    }

    #[test]
    fn local_time_at_the_minimum_time_value_resolves() {
        let sao_paulo = parse_zone("America/Sao_Paulo").expect("zone");
        // 2h before the minimum time value, in wall-clock terms: UTC(local) is still in range.
        let local = -8.64e15 - 2.0 * 3_600_000.0;
        let utc = local - offset_from_local(sao_paulo, local);
        assert!(utc >= -8.64e15, "{utc}");
    }

    #[test]
    fn zone_names_follow_the_season() {
        let name = |id: &str, t: f64| crate::intl::zone_long_name(id, t);
        // 2024-01-05T15:04:09Z and 2024-07-05T15:04:09Z.
        let (winter, summer) = (1_704_467_049_000.0, 1_720_191_849_000.0);
        assert_eq!(
            name("America/Sao_Paulo", winter).as_deref(),
            Some("Brasilia Standard Time")
        );
        assert_eq!(
            name("America/New_York", winter).as_deref(),
            Some("Eastern Standard Time")
        );
        assert_eq!(
            name("America/New_York", summer).as_deref(),
            Some("Eastern Daylight Time")
        );
        assert_eq!(
            name("UTC", winter).as_deref(),
            Some("Coordinated Universal Time")
        );
    }
}

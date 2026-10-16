//! The local time zone of `Date` (VELYR-114): `TZ` when it names an IANA zone, else the
//! host zone (`iana-time-zone`), else UTC; offsets come from the tzdb compiled into
//! `temporal_rs`.

use std::rc::Rc;

use temporal_rs::{TimeZone, ZonedDateTime};
use velyr_engine_vm::interpreter::{Interpreter, LocalTimeZone};

const MS_PER_DAY: f64 = 86_400_000.0;

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
    let before = offset_ms(zone, local - MS_PER_DAY);
    let after = offset_ms(zone, local + MS_PER_DAY);
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
        iana_time_zone::get_timezone()
            .ok()
            .and_then(|id| parse_zone(&id))
    };
    from_env.or_else(from_host).unwrap_or_else(TimeZone::utc)
}

struct HostZone(TimeZone);

impl LocalTimeZone for HostZone {
    fn offset_from_utc(&self, t: f64) -> f64 {
        offset_ms(self.0, t)
    }

    fn offset_from_local(&self, t: f64) -> f64 {
        offset_from_local(self.0, t)
    }

    /// shortcut: no zone display name (V8 prints the CLDR long name, e.g. `Brasilia
    /// Standard Time`), so `toString` ends at the offset; add one via ICU4X zone names.
    fn name(&self, _t: f64) -> Option<String> {
        None
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
}

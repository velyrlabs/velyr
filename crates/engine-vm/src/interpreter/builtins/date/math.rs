//! Time values (ECMA-262 §21.4.1): days, years, months and the Make* operations on
//! milliseconds since the epoch. All functions are total over finite inputs; NaN flows
//! through like in the spec.

pub(crate) const MS_PER_DAY: f64 = 86_400_000.0;
pub(crate) const MS_PER_HOUR: f64 = 3_600_000.0;
pub(crate) const MS_PER_MINUTE: f64 = 60_000.0;
pub(crate) const MS_PER_SECOND: f64 = 1_000.0;
/// ±8.64e15 ms: the range of valid time values.
pub(crate) const MAX_TIME: f64 = 8.64e15;

/// Euclidean remainder without `-0` (which would print as `-0`).
fn modulo(a: f64, b: f64) -> f64 {
    a.rem_euclid(b) + 0.0
}

pub(crate) fn day(t: f64) -> f64 {
    (t / MS_PER_DAY).floor()
}

pub(crate) fn time_within_day(t: f64) -> f64 {
    modulo(t, MS_PER_DAY)
}

fn in_leap_year(y: f64) -> bool {
    (y % 4.0 == 0.0 && y % 100.0 != 0.0) || y % 400.0 == 0.0
}

fn day_from_year(y: f64) -> f64 {
    365.0 * (y - 1970.0) + ((y - 1969.0) / 4.0).floor() - ((y - 1901.0) / 100.0).floor()
        + ((y - 1601.0) / 400.0).floor()
}

pub(crate) fn year_from_time(t: f64) -> f64 {
    let d = day(t);
    let mut y = (d / 365.2425).floor() + 1970.0;
    while day_from_year(y) > d {
        y -= 1.0;
    }
    while day_from_year(y + 1.0) <= d {
        y += 1.0;
    }
    y
}

/// Cumulative days before each month of a common year.
const MONTH_STARTS: [f64; 13] = [
    0.0, 31.0, 59.0, 90.0, 120.0, 151.0, 181.0, 212.0, 243.0, 273.0, 304.0, 334.0, 365.0,
];

fn month_start(month: usize, leap: bool) -> f64 {
    MONTH_STARTS[month] + if leap && month >= 2 { 1.0 } else { 0.0 }
}

/// (year, month 0-11, date 1-31) of a time value.
pub(crate) fn year_month_date(t: f64) -> (f64, f64, f64) {
    let year = year_from_time(t);
    let in_year = day(t) - day_from_year(year);
    let leap = in_leap_year(year);
    let month = (1..12)
        .find(|&m| in_year < month_start(m, leap))
        .map_or(11, |m| m - 1);
    (year, month as f64, in_year - month_start(month, leap) + 1.0)
}

pub(crate) fn week_day(t: f64) -> f64 {
    modulo(day(t) + 4.0, 7.0)
}

pub(crate) fn hour_from_time(t: f64) -> f64 {
    (time_within_day(t) / MS_PER_HOUR).floor()
}

pub(crate) fn min_from_time(t: f64) -> f64 {
    modulo((t / MS_PER_MINUTE).floor(), 60.0)
}

pub(crate) fn sec_from_time(t: f64) -> f64 {
    modulo((t / MS_PER_SECOND).floor(), 60.0)
}

pub(crate) fn ms_from_time(t: f64) -> f64 {
    modulo(t, MS_PER_SECOND)
}

fn to_integer(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else { n.trunc() }
}

pub(crate) fn make_time(hour: f64, min: f64, sec: f64, ms: f64) -> f64 {
    if ![hour, min, sec, ms].iter().all(|n| n.is_finite()) {
        return f64::NAN;
    }
    to_integer(hour) * MS_PER_HOUR
        + to_integer(min) * MS_PER_MINUTE
        + to_integer(sec) * MS_PER_SECOND
        + to_integer(ms)
}

pub(crate) fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if ![year, month, date].iter().all(|n| n.is_finite()) {
        return f64::NAN;
    }
    let (y, m, dt) = (to_integer(year), to_integer(month), to_integer(date));
    let ym = y + (m / 12.0).floor();
    if ym.abs() > 400_000.0 {
        return f64::NAN;
    }
    let mn = modulo(m, 12.0) as usize;
    day_from_year(ym) + month_start(mn, in_leap_year(ym)) + dt - 1.0
}

pub(crate) fn make_date(day: f64, time: f64) -> f64 {
    if !day.is_finite() || !time.is_finite() {
        return f64::NAN;
    }
    let tv = day * MS_PER_DAY + time;
    if tv.is_finite() { tv } else { f64::NAN }
}

pub(crate) fn time_clip(time: f64) -> f64 {
    if !time.is_finite() || time.abs() > MAX_TIME {
        return f64::NAN;
    }
    to_integer(time) + 0.0
}

/// MakeDate(MakeDay(y, m, d), MakeTime(h, min, s, ms)), with the two-digit year rule
/// left to the caller.
pub(crate) fn from_fields(fields: [f64; 7]) -> f64 {
    let [y, m, d, h, min, s, ms] = fields;
    make_date(make_day(y, m, d), make_time(h, min, s, ms))
}

/// The seven fields of a finite time value.
pub(crate) fn to_fields(t: f64) -> [f64; 7] {
    let (y, m, d) = year_month_date(t);
    [
        y,
        m,
        d,
        hour_from_time(t),
        min_from_time(t),
        sec_from_time(t),
        ms_from_time(t),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip_across_eras() {
        for t in [
            0.0,
            -1.0,
            951_782_400_000.0,
            -62_198_755_200_000.0,
            MAX_TIME,
            -MAX_TIME,
        ] {
            assert_eq!(from_fields(to_fields(t)), t, "{t}");
        }
        assert_eq!(to_fields(951_782_400_000.0)[..3], [2000.0, 1.0, 29.0]);
        assert_eq!(week_day(0.0), 4.0);
        assert!(make_day(f64::NAN, 0.0, 1.0).is_nan());
    }
}

//! `Intl.DateTimeFormat` over ICU4X's dynamic field sets. The prelude resolves the
//! options (ToDateTimeOptions, style/component conflicts) into a `spec`; the instant is
//! moved to the wall clock of the time zone with the tzdb of `temporal_rs`.

use icu_calendar::Iso;
use icu_datetime::fieldsets::builder::{DateFields, FieldSetBuilder, ZoneStyle};
use icu_datetime::fieldsets::enums::{CompositeFieldSet, ZoneFieldSet};
use icu_datetime::input::{Date, DateTime, Time, TimeZone, UtcOffset, ZonedDateTime};
use icu_datetime::options::{Length, SubsecondDigits, TimePrecision, YearStyle};
use icu_datetime::preferences::HourCycle;
use icu_datetime::{DateTimeFormatter, DateTimeFormatterPreferences, parts};
use temporal_rs::ZonedDateTime as TemporalZoned;
use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};
use writeable::{Part, Writeable};

use super::{OpResult, Parts, bool_arg, locale, number_arg, range_error, string_arg};
use crate::date::{host_zone, parse_zone};

/// Option names in the order the prelude joins them into the spec.
const KEYS: [&str; 13] = [
    "dateStyle",
    "timeStyle",
    "weekday",
    "era",
    "year",
    "month",
    "day",
    "hour",
    "minute",
    "second",
    "fractionalSecondDigits",
    "hourCycle",
    "timeZoneName",
];

struct Spec(Vec<String>);

impl Spec {
    fn get(&self, key: &str) -> Option<&str> {
        let index = KEYS.iter().position(|k| *k == key)?;
        self.0
            .get(index)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }
}

fn length(style: &str) -> Length {
    match style {
        "full" | "long" => Length::Long,
        "medium" => Length::Medium,
        _ => Length::Short,
    }
}

fn date_fields(spec: &Spec) -> Option<DateFields> {
    let has = |key| spec.get(key).is_some();
    Some(
        match (has("year"), has("month"), has("day"), has("weekday")) {
            (true, _, true, true) => DateFields::YMDE,
            (true, _, true, false) => DateFields::YMD,
            (false, true, true, true) => DateFields::MDE,
            (false, true, true, false) => DateFields::MD,
            (true, true, false, _) => DateFields::YM,
            (true, false, false, _) => DateFields::Y,
            (false, true, false, _) => DateFields::M,
            (false, false, true, true) => DateFields::DE,
            (false, false, true, false) => DateFields::D,
            (false, false, false, true) => DateFields::E,
            (false, false, false, false) => return None,
        },
    )
}

fn time_precision(spec: &Spec) -> Option<TimePrecision> {
    spec.get("hour")?;
    Some(
        match (
            spec.get("minute"),
            spec.get("second"),
            spec.get("fractionalSecondDigits"),
        ) {
            (_, Some(_), Some("1")) => TimePrecision::Subsecond(SubsecondDigits::S1),
            (_, Some(_), Some("2")) => TimePrecision::Subsecond(SubsecondDigits::S2),
            (_, Some(_), Some(_)) => TimePrecision::Subsecond(SubsecondDigits::S3),
            (_, Some(_), None) => TimePrecision::Second,
            (Some(_), None, _) => TimePrecision::Minute,
            (None, None, _) => TimePrecision::Hour,
        },
    )
}

fn zone_style(spec: &Spec) -> Option<ZoneStyle> {
    if spec.get("dateStyle").is_some() || spec.get("timeStyle").is_some() {
        return match spec.get("timeStyle") {
            Some("full") => Some(ZoneStyle::SpecificLong),
            Some("long") => Some(ZoneStyle::SpecificShort),
            _ => None,
        };
    }
    Some(match spec.get("timeZoneName")? {
        "short" => ZoneStyle::SpecificShort,
        "long" => ZoneStyle::SpecificLong,
        "shortOffset" => ZoneStyle::LocalizedOffsetShort,
        "longOffset" => ZoneStyle::LocalizedOffsetLong,
        "shortGeneric" => ZoneStyle::GenericShort,
        _ => ZoneStyle::GenericLong,
    })
}

/// shortcut: one length for the whole pattern (ICU4X field sets), so `dateStyle` and
/// `timeStyle` of different lengths use the date's (which also picks the date-time glue,
/// as CLDR does), and textual weekday/month lengths collapse onto it; the narrow forms
/// are not available.
fn field_set(spec: &Spec) -> Option<CompositeFieldSet> {
    let mut builder = FieldSetBuilder::new();
    builder.zone_style = zone_style(spec);
    if spec.get("dateStyle").is_some() || spec.get("timeStyle").is_some() {
        if let Some(style) = spec.get("dateStyle") {
            builder.length = Some(length(style));
            builder.date_fields = Some(if style == "full" {
                DateFields::YMDE
            } else {
                DateFields::YMD
            });
        }
        if let Some(style) = spec.get("timeStyle") {
            builder.length.get_or_insert(length(style));
            builder.time_precision = Some(if style == "short" {
                TimePrecision::Minute
            } else {
                TimePrecision::Second
            });
        }
    } else {
        builder.date_fields = date_fields(spec);
        builder.time_precision = time_precision(spec);
        builder.length = Some(match (spec.get("month"), spec.get("weekday")) {
            (Some("long"), _) => Length::Long,
            (Some("short" | "narrow"), _) => Length::Medium,
            (None, Some("long")) => Length::Long,
            _ => Length::Short,
        });
        let has_year = matches!(
            builder.date_fields,
            Some(DateFields::Y | DateFields::YM | DateFields::YMD | DateFields::YMDE)
        );
        if has_year && spec.get("era").is_some() {
            builder.year_style = Some(YearStyle::WithEra);
        } else if has_year && spec.get("year") == Some("numeric") {
            builder.year_style = Some(YearStyle::Full);
        }
    }
    builder.build_composite().ok()
}

fn preferences(tag: &str, hour_cycle: Option<&str>) -> DateTimeFormatterPreferences {
    let mut prefs = DateTimeFormatterPreferences::from(&locale(tag));
    prefs.hour_cycle = match hour_cycle {
        Some("h11") => Some(HourCycle::H11),
        Some("h12") => Some(HourCycle::H12),
        Some("h23" | "h24") => Some(HourCycle::H23),
        _ => prefs.hour_cycle,
    };
    prefs
}

/// The instant `t` (ms) as the wall clock of the time zone `zone`.
struct Instant {
    wall: DateTime<Iso>,
    zone: TimeZone,
    offset: Option<UtcOffset>,
}

fn instant(zone: &str, t: f64) -> Option<Instant> {
    let id = parse_zone(zone)?;
    let zdt = TemporalZoned::try_new_iso(t as i128 * 1_000_000, id).ok()?;
    let date = Date::try_new_iso(zdt.year(), zdt.month(), zdt.day()).ok()?;
    let nanos = u32::from(zdt.millisecond()) * 1_000_000;
    let time = Time::try_new(zdt.hour(), zdt.minute(), zdt.second(), nanos).ok()?;
    let seconds = (zdt.offset_nanoseconds() / 1_000_000_000) as i32;
    // Offset zones (`+05:30`) have no IANA name to look up.
    let named = !zone.starts_with(['+', '-']);
    Some(Instant {
        wall: DateTime { date, time },
        zone: if named {
            TimeZone::from_iana_id(zone)
        } else {
            TimeZone::UNKNOWN
        },
        offset: UtcOffset::try_from_seconds(seconds).ok(),
    })
}

fn part_name(part: Part) -> &'static str {
    match part {
        p if p == parts::ERA => "era",
        p if p == parts::YEAR => "year",
        p if p == parts::RELATED_YEAR => "relatedYear",
        p if p == parts::YEAR_NAME => "yearName",
        p if p == parts::MONTH => "month",
        p if p == parts::DAY => "day",
        p if p == parts::WEEKDAY => "weekday",
        p if p == parts::DAY_PERIOD => "dayPeriod",
        p if p == parts::HOUR => "hour",
        p if p == parts::MINUTE => "minute",
        p if p == parts::SECOND => "second",
        p if p == parts::TIME_ZONE_NAME => "timeZoneName",
        _ => "literal",
    }
}

/// The weekday name at its own length: the field sets share one length with the month, so
/// `weekday: 'long', month: 'short'` would otherwise get an abbreviated weekday.
fn weekday_text(
    prefs: DateTimeFormatterPreferences,
    style: &str,
    date: &Date<Iso>,
) -> Option<String> {
    let mut builder = FieldSetBuilder::new();
    builder.date_fields = Some(DateFields::E);
    builder.length = Some(if style == "long" {
        Length::Long
    } else {
        Length::Medium
    });
    let formatter = DateTimeFormatter::try_new(prefs, builder.build_date().ok()?).ok()?;
    Some(formatter.format(date).write_to_string().into_owned())
}

/// Pads or truncates the numeric fields requested as `2-digit`; ICU4X patterns only know
/// the locale's own width. Non-Latin digits are left as formatted.
fn apply_two_digit(spec: &Spec, parts: &mut Vec<(&'static str, String)>) {
    // ICU4X writes the digits of one field as separate entries; join them first.
    let mut merged: Vec<(&'static str, String)> = Vec::with_capacity(parts.len());
    for (kind, text) in parts.drain(..) {
        match merged.last_mut() {
            Some((last, buffer)) if *last == kind => buffer.push_str(&text),
            _ => merged.push((kind, text)),
        }
    }
    *parts = merged;
    for (kind, text) in parts {
        if spec.get(kind) != Some("2-digit") || !text.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if *kind == "year" {
            text.drain(..text.len().saturating_sub(2));
        } else if text.len() < 2 {
            text.insert(0, '0');
        }
    }
}

fn failure(vm: &mut Interpreter, what: &str) -> Abrupt {
    range_error(vm, &format!("Cannot format the date: {what}"))
}

/// `(locale, spec, epoch ms, time zone, parts)`: the formatted date, or encoded parts.
pub(crate) fn format(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let spec = Spec(string_arg(args, 1).split(',').map(str::to_owned).collect());
    let Some(fields) = field_set(&spec) else {
        return Err(failure(vm, "unsupported option combination"));
    };
    let prefs = preferences(&string_arg(args, 0), spec.get("hourCycle"));
    let Ok(formatter) = DateTimeFormatter::try_new(prefs, fields) else {
        return Err(failure(vm, "no data for this locale"));
    };
    let Some(at) = instant(&string_arg(args, 3), number_arg(args, 2)) else {
        return Err(failure(vm, "time value out of range"));
    };
    let zone = at.zone.with_offset(at.offset).at_date_time(at.wall);
    let input = ZonedDateTime {
        date: at.wall.date,
        time: at.wall.time,
        zone,
    };
    let mut out = Parts::new(part_name);
    let _ = formatter.format(&input).write_to_parts(&mut out);
    apply_two_digit(&spec, &mut out.parts);
    // `weekday` and `dateStyle` never combine (the prelude rejects it).
    if let Some(style) = spec.get("weekday")
        && let Some(name) = weekday_text(prefs, style, &at.wall.date)
    {
        for (_, text) in out.parts.iter_mut().filter(|(kind, _)| *kind == "weekday") {
            text.clone_from(&name);
        }
    }
    if bool_arg(args, 4) {
        return Ok(Value::str(&out.encode()));
    }
    let text: String = out.parts.iter().map(|(_, text)| text.as_str()).collect();
    Ok(Value::str(&text))
}

thread_local! {
    static ZONE_NAMES: DateTimeFormatter<ZoneFieldSet> = {
        let mut builder = FieldSetBuilder::new();
        builder.zone_style = Some(ZoneStyle::SpecificLong);
        let fields = builder.build_zone().expect("a zone style builds a zone field set");
        let prefs = DateTimeFormatterPreferences::from(&locale("en-US"));
        DateTimeFormatter::try_new(prefs, fields).expect("compiled data has the zone names")
    };
}

/// The English long name of `zone` at `t` (`Brasilia Standard Time`), as printed by
/// `Date.prototype.toString`.
pub(crate) fn zone_long_name(zone: &str, t: f64) -> Option<String> {
    let at = instant(zone, t)?;
    let info = at.zone.with_offset(at.offset).at_date_time(at.wall);
    ZONE_NAMES.with(|formatter| Some(formatter.format(&info).write_to_string().into_owned()))
}

/// The canonical name of a time zone, or `undefined` when it is not one.
pub(crate) fn canonical_zone(_: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let id = parse_zone(&string_arg(args, 0)).and_then(|zone| zone.identifier().ok());
    Ok(id.map_or(Value::Undefined, |id| Value::str(&id)))
}

pub(crate) fn default_zone(_: &mut Interpreter, _: &Value, _: &[Value]) -> OpResult {
    let id = host_zone().identifier().unwrap_or_else(|_| "UTC".into());
    Ok(Value::str(&id))
}

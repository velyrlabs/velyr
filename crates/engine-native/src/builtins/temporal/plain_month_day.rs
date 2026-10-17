//! `Temporal.PlainMonthDay` ops; the state is `isoMonth,isoDay,referenceYear` (the reference
//! year is kept apart from the string form because it is not always 1972).

use temporal_rs::fields::CalendarFields;
use temporal_rs::options::{DisplayCalendar, Overflow};
use temporal_rs::partial::PartialDate;
use temporal_rs::{Calendar, MonthCode, PlainMonthDay, TemporalError, TemporalResult};

use super::{Args, Res, arg, ensure_iso, num, overflow, range};

fn state(value: &PlainMonthDay) -> String {
    format!(
        "{},{},{}",
        value.month_code().to_month_integer(),
        value.day(),
        value.reference_year()
    )
}

fn parse_state(text: &str) -> TemporalResult<PlainMonthDay> {
    let parts: Vec<&str> = text.split(',').collect();
    let [month, day, year] = parts.as_slice() else {
        return Err(range("invalid PlainMonthDay state"));
    };
    let bad = || range("invalid PlainMonthDay state");
    PlainMonthDay::new_with_overflow(
        month.parse().map_err(|_| bad())?,
        day.parse().map_err(|_| bad())?,
        Calendar::ISO,
        Overflow::Reject,
        Some(year.parse().map_err(|_| bad())?),
    )
}

fn month_day(a: Args, i: usize) -> TemporalResult<PlainMonthDay> {
    parse_state(arg(a, i))
}

/// Calendar fields from `year,month,monthCode,day` args starting at `i`; `""` is absent.
fn fields(a: Args, i: usize) -> TemporalResult<CalendarFields> {
    let present = |n: usize| (!arg(a, i + n).is_empty()).then_some(n);
    let mut fields = CalendarFields::new();
    if present(0).is_some() {
        fields.year = Some(num(a, i, i32::MAX.into())?);
    }
    if present(1).is_some() {
        fields.month = Some(num(a, i + 1, 255)?);
    }
    if present(2).is_some() {
        fields.month_code = Some(MonthCode::try_from_utf8(arg(a, i + 2).as_bytes())?);
    }
    if present(3).is_some() {
        fields.day = Some(num(a, i + 3, 255)?);
    }
    Ok(fields)
}

pub(super) fn run(method: &str, a: Args) -> Res {
    match method {
        "parse" => {
            let value = PlainMonthDay::from_utf8(arg(a, 0).as_bytes())?;
            ensure_iso(value.calendar())?;
            Ok(state(&value))
        }
        "create" => PlainMonthDay::new_with_overflow(
            num(a, 0, 255)?,
            num(a, 1, 255)?,
            Calendar::ISO,
            overflow(a, 3)?.unwrap_or(Overflow::Reject),
            Some(num(a, 2, i32::MAX.into())?),
        )
        .map(|value| state(&value)),
        "fromFields" => {
            let partial = PartialDate {
                calendar_fields: fields(a, 0)?,
                calendar: Calendar::ISO,
            };
            PlainMonthDay::from_partial(partial, overflow(a, 4)?).map(|value| state(&value))
        }
        "with" => {
            let fields = fields(a, 1)?;
            month_day(a, 0)?
                .with(fields, overflow(a, 5)?)
                .map(|value| state(&value))
        }
        "fields" => {
            let value = month_day(a, 0)?;
            Ok(format!("{},{}", value.month_code().as_str(), value.day()))
        }
        "toString" => {
            let display = match arg(a, 1) {
                "always" => DisplayCalendar::Always,
                "never" => DisplayCalendar::Never,
                "critical" => DisplayCalendar::Critical,
                _ => DisplayCalendar::Auto,
            };
            Ok(month_day(a, 0)?.to_ixdtf_string(display))
        }
        "equals" => Ok((month_day(a, 0)? == month_day(a, 1)?).to_string()),
        "toDate" => {
            let year = CalendarFields::new().with_year(num(a, 1, i32::MAX.into())?);
            let date = month_day(a, 0)?.to_plain_date(Some(year))?;
            Ok(date.to_ixdtf_string(DisplayCalendar::Auto))
        }
        _ => Err(TemporalError::general("unknown PlainMonthDay op")),
    }
}

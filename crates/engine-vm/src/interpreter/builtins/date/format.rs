//! String forms of a time value (ECMA-262 §21.4.4.35-43): `toString` and its date/time
//! halves, `toUTCString`, `toISOString` and `toJSON`.

use super::math::{MS_PER_MINUTE, to_fields, week_day};
use super::this_time;
use crate::Hint;
use crate::Value;
use crate::interpreter::builtins::{Interpreter, JsResult};

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const INVALID: &str = "Invalid Date";

/// `-0001`, `0001`, `275760`.
fn year_string(year: f64) -> String {
    let sign = if year < 0.0 { "-" } else { "" };
    format!("{sign}{:04}", year.abs() as i64)
}

/// `Thu Jan 01 1970` of a local time.
fn date_part(local: f64) -> String {
    let [year, month, date, ..] = to_fields(local);
    let day = DAYS[week_day(local) as usize];
    let month = MONTHS[month as usize];
    format!("{day} {month} {date:02} {}", year_string(year))
}

/// `00:00:00 GMT+0000 (Coordinated Universal Time)` of the instant `t`.
fn time_part(vm: &Interpreter, t: f64) -> String {
    let local = vm.local_time(t);
    let [.., hour, minute, second, _] = to_fields(local);
    let offset = (local - t) / MS_PER_MINUTE;
    let sign = if offset < 0.0 { '-' } else { '+' };
    let (hh, mm) = ((offset.abs() / 60.0).floor(), offset.abs() % 60.0);
    let name = vm
        .zone_name(t)
        .map(|name| format!(" ({name})"))
        .unwrap_or_default();
    format!("{hour:02}:{minute:02}:{second:02} GMT{sign}{hh:02}{mm:02}{name}")
}

pub(super) fn date_to_string(vm: &Interpreter, t: f64) -> JsResult<String> {
    if t.is_nan() {
        return Ok(INVALID.to_owned());
    }
    Ok(format!(
        "{} {}",
        date_part(vm.local_time(t)),
        time_part(vm, t)
    ))
}

fn text(s: &str) -> JsResult<Value> {
    Ok(Value::str(s))
}

pub(super) fn to_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(vm, this)?;
    text(&date_to_string(vm, t)?)
}

pub(super) fn to_date_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(vm, this)?;
    if t.is_nan() {
        return text(INVALID);
    }
    text(&date_part(vm.local_time(t)))
}

pub(super) fn to_time_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(vm, this)?;
    if t.is_nan() {
        return text(INVALID);
    }
    text(&time_part(vm, t))
}

pub(super) fn to_utc_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(vm, this)?;
    if t.is_nan() {
        return text(INVALID);
    }
    let [year, month, date, hour, minute, second, _] = to_fields(t);
    let (day, month) = (DAYS[week_day(t) as usize], MONTHS[month as usize]);
    let year = year_string(year);
    text(&format!(
        "{day}, {date:02} {month} {year} {hour:02}:{minute:02}:{second:02} GMT"
    ))
}

pub(super) fn to_iso_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(vm, this)?;
    if t.is_nan() {
        return vm.throw_range("Invalid time value");
    }
    let [year, month, date, hour, minute, second, ms] = to_fields(t);
    let year = if (0.0..=9999.0).contains(&year) {
        format!("{:04}", year as i64)
    } else {
        let sign = if year < 0.0 { '-' } else { '+' };
        format!("{sign}{:06}", year.abs() as i64)
    };
    let month = month + 1.0;
    text(&format!(
        "{year}-{month:02}-{date:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z"
    ))
}

pub(super) fn to_json(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let object = vm.coerce_object(this)?;
    let primitive = vm.coerce_primitive(this, Hint::Number)?;
    if let Value::Number(n) = primitive
        && !n.is_finite()
    {
        return Ok(Value::Null);
    }
    let method = vm.get_named(object, "toISOString")?;
    if !vm.is_callable(&method) {
        return vm.throw_type("toISOString is not a function");
    }
    vm.call(&method, this, &[])
}

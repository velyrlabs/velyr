//! The `Date` constructor and prototype (ECMA-262 §21.4, Annex B `getYear`/`setYear`/
//! `toGMTString`). Local time goes through the embedder's [`LocalTimeZone`]; without one
//! the local zone is UTC. `toLocale*String` print like `toString` here; an embedder with
//! Intl replaces them.

#[path = "date/format.rs"]
mod format;
#[path = "date/math.rs"]
mod math;
#[path = "date/parse.rs"]
mod parse;

use std::rc::Rc;

use self::math::{MS_PER_MINUTE, from_fields, time_clip, to_fields, week_day};
use super::slots::Slot;
use super::symbol::SYMBOL_TO_PRIMITIVE;
use super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::Hint;
use crate::{PropertyDescriptor, Value};

/// The host's local time zone; offsets are in milliseconds, east of UTC positive.
pub trait LocalTimeZone {
    /// Offset at the instant `t` (ms since the epoch, UTC).
    fn offset_from_utc(&self, t: f64) -> f64;
    /// Offset to subtract from the local wall-clock time `t` (ms since the epoch as if it
    /// were UTC) to get UTC; gaps and overlaps resolve like Temporal's `compatible`.
    fn offset_from_local(&self, t: f64) -> f64;
    /// Display name for `toString`, e.g. `Brasilia Standard Time`.
    fn name(&self, t: f64) -> Option<String>;
}

impl Interpreter {
    /// Sets the local time zone of `Date`.
    pub fn set_local_time_zone(&mut self, zone: Rc<dyn LocalTimeZone>) {
        self.local_time_zone = Some(zone);
    }

    fn offset_at(&self, t: f64) -> f64 {
        self.local_time_zone
            .as_ref()
            .map_or(0.0, |zone| zone.offset_from_utc(t))
    }

    /// LocalTime(t).
    fn local_time(&self, t: f64) -> f64 {
        t + self.offset_at(t)
    }

    /// UTC(t) of a local time.
    fn utc(&self, t: f64) -> f64 {
        if !t.is_finite() {
            return f64::NAN;
        }
        t - self
            .local_time_zone
            .as_ref()
            .map_or(0.0, |zone| zone.offset_from_local(t))
    }

    fn zone_name(&self, t: f64) -> Option<String> {
        self.local_time_zone.as_ref().and_then(|zone| zone.name(t))
    }
}

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::DatePrototype);
    let constructor = vm.constructor("Date", 7, date_constructor, proto)?;
    vm.method(constructor, "now", 0, date_now)?;
    vm.method(constructor, "parse", 1, date_parse)?;
    vm.method(constructor, "UTC", 7, date_utc)?;
    for (name, arity, f) in METHODS {
        vm.method(proto, name, *arity, *f)?;
    }
    let to_utc_string = vm.get_named(proto, "toUTCString")?;
    vm.define_method(proto, "toGMTString", to_utc_string)?;
    let to_primitive = vm.new_builtin("[Symbol.toPrimitive]", 1, date_to_primitive)?;
    let desc = PropertyDescriptor::data(to_primitive, false, false, true);
    let key = crate::ObjectKey::Symbol(SYMBOL_TO_PRIMITIVE);
    vm.define_property(proto, key, &desc)?;
    Ok(())
}

type Method = (&'static str, u32, super::Builtin);

const METHODS: &[Method] = &[
    ("getDate", 0, get_date),
    ("getDay", 0, get_day),
    ("getFullYear", 0, get_full_year),
    ("getHours", 0, get_hours),
    ("getMilliseconds", 0, get_milliseconds),
    ("getMinutes", 0, get_minutes),
    ("getMonth", 0, get_month),
    ("getSeconds", 0, get_seconds),
    ("getTime", 0, get_time),
    ("getTimezoneOffset", 0, get_timezone_offset),
    ("getUTCDate", 0, get_utc_date),
    ("getUTCDay", 0, get_utc_day),
    ("getUTCFullYear", 0, get_utc_full_year),
    ("getUTCHours", 0, get_utc_hours),
    ("getUTCMilliseconds", 0, get_utc_milliseconds),
    ("getUTCMinutes", 0, get_utc_minutes),
    ("getUTCMonth", 0, get_utc_month),
    ("getUTCSeconds", 0, get_utc_seconds),
    ("getYear", 0, get_year),
    ("setDate", 1, set_date),
    ("setFullYear", 3, set_full_year),
    ("setHours", 4, set_hours),
    ("setMilliseconds", 1, set_milliseconds),
    ("setMinutes", 3, set_minutes),
    ("setMonth", 2, set_month),
    ("setSeconds", 2, set_seconds),
    ("setTime", 1, set_time),
    ("setUTCDate", 1, set_utc_date),
    ("setUTCFullYear", 3, set_utc_full_year),
    ("setUTCHours", 4, set_utc_hours),
    ("setUTCMilliseconds", 1, set_utc_milliseconds),
    ("setUTCMinutes", 3, set_utc_minutes),
    ("setUTCMonth", 2, set_utc_month),
    ("setUTCSeconds", 2, set_utc_seconds),
    ("setYear", 1, set_year),
    ("toDateString", 0, format::to_date_string),
    ("toISOString", 0, format::to_iso_string),
    ("toJSON", 1, format::to_json),
    ("toLocaleDateString", 0, format::to_date_string),
    ("toLocaleString", 0, format::to_string),
    ("toLocaleTimeString", 0, format::to_time_string),
    ("toString", 0, format::to_string),
    ("toTimeString", 0, format::to_time_string),
    ("toUTCString", 0, format::to_utc_string),
    ("valueOf", 0, get_time),
];

fn now() -> f64 {
    let elapsed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    elapsed.map_or(0.0, |d| d.as_millis() as f64)
}

/// thisTimeValue.
fn this_time(vm: &mut Interpreter, this: &Value) -> JsResult<f64> {
    match vm.value_slot(this) {
        Some(Slot::Date(t)) => Ok(t),
        _ => vm.throw_type("this is not a Date object."),
    }
}

fn set_this_time(vm: &mut Interpreter, this: &Value, t: f64) -> JsResult<Value> {
    if let Some(object) = vm.as_object(this) {
        vm.set_slot(object, Slot::Date(t));
    }
    Ok(Value::Number(t))
}

/// The two-digit year rule of the constructor, `Date.UTC` and `setYear`.
fn full_year(y: f64) -> f64 {
    let integer = y.trunc();
    if !y.is_nan() && (0.0..=99.0).contains(&integer) {
        1900.0 + integer
    } else {
        y
    }
}

/// MakeDate over the constructor/`Date.UTC` arguments (year, month, [date, h, m, s, ms]).
fn fields_from_args(vm: &mut Interpreter, args: &[Value]) -> JsResult<f64> {
    let mut fields = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
    for (i, value) in args.iter().take(7).enumerate() {
        fields[i] = vm.coerce_number(value)?;
    }
    fields[0] = full_year(fields[0]);
    Ok(from_fields(fields))
}

fn date_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        let t = now();
        return format::date_to_string(vm, t).map(|s| Value::str(&s));
    }
    let t = match args {
        [] => now(),
        [value] => match vm.value_slot(value) {
            Some(Slot::Date(t)) => t,
            _ => match vm.coerce_primitive(value, Hint::Default)? {
                Value::String(text) => parse_time(vm, &text.to_string_lossy()),
                primitive => vm.coerce_number(&primitive)?,
            },
        },
        _ => {
            let local = fields_from_args(vm, args)?;
            vm.utc(local)
        }
    };
    let proto = vm.intrinsic(Intrinsic::DatePrototype);
    let slot = Some(Slot::Date(time_clip(t)));
    Ok(object_value(vm.create_from_new_target(
        &new_target,
        proto,
        slot,
    )?))
}

fn parse_time(vm: &Interpreter, text: &str) -> f64 {
    match parse::parse(text) {
        Some(parse::Parsed::Utc(t)) => t,
        Some(parse::Parsed::Local(t)) => vm.utc(t),
        None => f64::NAN,
    }
}

fn date_now(_: &mut Interpreter, _: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(now()))
}

fn date_parse(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let text = vm.coerce_string(&arg(args, 0))?;
    Ok(Value::Number(time_clip(parse_time(
        vm,
        &text.to_string_lossy(),
    ))))
}

fn date_utc(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let t = fields_from_args(
        vm,
        if args.is_empty() {
            &[Value::Undefined]
        } else {
            args
        },
    )?;
    Ok(Value::Number(time_clip(t)))
}

/// Getters: the field `pick` of the local (`true`) or UTC time.
macro_rules! getters {
    ($($name:ident: $local:expr, $pick:expr;)*) => {$(
        fn $name(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
            let t = this_time(vm, this)?;
            if t.is_nan() {
                return Ok(Value::Number(f64::NAN));
            }
            let t = if $local { vm.local_time(t) } else { t };
            let pick: fn(f64) -> f64 = $pick;
            Ok(Value::Number(pick(t)))
        }
    )*};
}

getters! {
    get_date: true, |t| to_fields(t)[2];
    get_day: true, week_day;
    get_full_year: true, |t| to_fields(t)[0];
    get_hours: true, |t| to_fields(t)[3];
    get_milliseconds: true, |t| to_fields(t)[6];
    get_minutes: true, |t| to_fields(t)[4];
    get_month: true, |t| to_fields(t)[1];
    get_seconds: true, |t| to_fields(t)[5];
    get_year: true, |t| to_fields(t)[0] - 1900.0;
    get_utc_date: false, |t| to_fields(t)[2];
    get_utc_day: false, week_day;
    get_utc_full_year: false, |t| to_fields(t)[0];
    get_utc_hours: false, |t| to_fields(t)[3];
    get_utc_milliseconds: false, |t| to_fields(t)[6];
    get_utc_minutes: false, |t| to_fields(t)[4];
    get_utc_month: false, |t| to_fields(t)[1];
    get_utc_seconds: false, |t| to_fields(t)[5];
}

fn get_time(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(this_time(vm, this)?))
}

fn get_timezone_offset(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(vm, this)?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number((t - vm.local_time(t)) / MS_PER_MINUTE))
}

/// Sets up to `max` fields from index `start` (year, month, date, h, min, s, ms) of the
/// local or UTC time; only the arguments passed are read.
fn set_fields(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    (start, max, local): (usize, usize, bool),
) -> JsResult<Value> {
    let t = this_time(vm, this)?;
    let mut values = Vec::with_capacity(max);
    for i in 0..args.len().clamp(1, max) {
        values.push(vm.coerce_number(&arg(args, i))?);
    }
    let base = match (t.is_nan(), start) {
        (true, 0) => 0.0,
        (true, _) => return Ok(Value::Number(f64::NAN)),
        (false, _) if local => vm.local_time(t),
        (false, _) => t,
    };
    let mut fields = to_fields(base);
    fields[start..start + values.len()].copy_from_slice(&values);
    let date = from_fields(fields);
    let u = if local { vm.utc(date) } else { date };
    set_this_time(vm, this, time_clip(u))
}

macro_rules! setters {
    ($($name:ident: $start:expr, $max:expr, $local:expr;)*) => {$(
        fn $name(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
            set_fields(vm, this, args, ($start, $max, $local))
        }
    )*};
}

setters! {
    set_full_year: 0, 3, true;
    set_month: 1, 2, true;
    set_date: 2, 1, true;
    set_hours: 3, 4, true;
    set_minutes: 4, 3, true;
    set_seconds: 5, 2, true;
    set_milliseconds: 6, 1, true;
    set_utc_full_year: 0, 3, false;
    set_utc_month: 1, 2, false;
    set_utc_date: 2, 1, false;
    set_utc_hours: 3, 4, false;
    set_utc_minutes: 4, 3, false;
    set_utc_seconds: 5, 2, false;
    set_utc_milliseconds: 6, 1, false;
}

fn set_time(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    this_time(vm, this)?;
    let t = vm.coerce_number(&arg(args, 0))?;
    set_this_time(vm, this, time_clip(t))
}

/// Annex B `setYear`.
fn set_year(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let t = this_time(vm, this)?;
    let year = vm.coerce_number(&arg(args, 0))?;
    if year.is_nan() {
        return set_this_time(vm, this, f64::NAN);
    }
    let mut fields = to_fields(if t.is_nan() { 0.0 } else { vm.local_time(t) });
    fields[0] = full_year(year);
    let u = vm.utc(from_fields(fields));
    set_this_time(vm, this, time_clip(u))
}

/// `Date.prototype[@@toPrimitive](hint)`: `default` means `string`.
fn date_to_primitive(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let Some(object) = vm.as_object(this) else {
        return vm.throw_type("Date.prototype[Symbol.toPrimitive] called on non-object");
    };
    let order = match arg(args, 0) {
        Value::String(hint) if hint.to_string_lossy() == "number" => ["valueOf", "toString"],
        Value::String(hint) if matches!(&*hint.to_string_lossy(), "string" | "default") => {
            ["toString", "valueOf"]
        }
        _ => return vm.throw_type("Invalid hint"),
    };
    for name in order {
        let method = vm.get_named(object, name)?;
        if vm.is_callable(&method) {
            let result = vm.call(&method, this, &[])?;
            if !matches!(result, Value::Object(_)) {
                return Ok(result);
            }
        }
    }
    vm.throw_type("Cannot convert object to primitive value")
}

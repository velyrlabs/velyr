//! `String.prototype` methods that read or slice code units: `at`, `charAt`,
//! `charCodeAt`, `codePointAt`, `indexOf`, `lastIndexOf`, `includes`, `startsWith`,
//! `endsWith`, `slice`, `substring`, `substr`, `concat`, `repeat`, `padStart`, `padEnd`,
//! `isWellFormed`, `toWellFormed`, `toString` and `valueOf`.

use super::super::abstract_ops::relative_index;
use super::super::{Interpreter, JsResult, arg};
use super::{code_point_size, string_value, this_units};
use crate::Value;

pub(super) fn install(vm: &mut Interpreter, proto: crate::ObjectRef) -> JsResult<()> {
    vm.method(proto, "at", 1, string_at)?;
    vm.method(proto, "charAt", 1, string_char_at)?;
    vm.method(proto, "charCodeAt", 1, string_char_code_at)?;
    vm.method(proto, "codePointAt", 1, string_code_point_at)?;
    vm.method(proto, "concat", 1, string_concat)?;
    vm.method(proto, "endsWith", 1, string_ends_with)?;
    vm.method(proto, "includes", 1, string_includes)?;
    vm.method(proto, "indexOf", 1, string_index_of)?;
    vm.method(proto, "isWellFormed", 0, string_is_well_formed)?;
    vm.method(proto, "lastIndexOf", 1, string_last_index_of)?;
    vm.method(proto, "padEnd", 1, string_pad_end)?;
    vm.method(proto, "padStart", 1, string_pad_start)?;
    vm.method(proto, "repeat", 1, string_repeat)?;
    vm.method(proto, "slice", 2, string_slice)?;
    vm.method(proto, "startsWith", 1, string_starts_with)?;
    vm.method(proto, "substr", 2, string_substr)?;
    vm.method(proto, "substring", 2, string_substring)?;
    vm.method(proto, "toString", 0, string_to_string)?;
    vm.method(proto, "toWellFormed", 0, string_to_well_formed)?;
    vm.method(proto, "valueOf", 0, string_value_of)
}

/// StringIndexOf: first index `>= from` where `search` occurs.
pub(crate) fn index_of(units: &[u16], search: &[u16], from: usize) -> Option<usize> {
    if search.is_empty() {
        return (from <= units.len()).then_some(from);
    }
    if search.len() > units.len() {
        return None;
    }
    (from..=units.len() - search.len()).find(|&i| units[i..].starts_with(search))
}

fn position_arg(
    vm: &mut Interpreter,
    args: &[Value],
    index: usize,
    length: usize,
) -> JsResult<usize> {
    let position = vm.coerce_integer(&arg(args, index))?;
    Ok(position.clamp(0.0, length as f64) as usize)
}

/// ToString of a search argument that must not be a RegExp (IsRegExp).
fn search_string(vm: &mut Interpreter, value: &Value, method: &str) -> JsResult<Vec<u16>> {
    if vm.is_regexp(value)? {
        let message =
            format!("First argument to String.prototype.{method} must not be a regular expression");
        return vm.throw_type(&message);
    }
    Ok(vm.coerce_string(value)?.to_utf16())
}

fn string_at(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "at")?;
    let relative = vm.coerce_integer(&arg(args, 0))?;
    let length = units.len() as f64;
    let k = if relative >= 0.0 {
        relative
    } else {
        length + relative
    };
    if k < 0.0 || k >= length {
        return Ok(Value::Undefined);
    }
    Ok(string_value(vec![units[k as usize]]))
}

/// The code unit index of `args[0]` when it is in range.
fn unit_index(vm: &mut Interpreter, units: &[u16], args: &[Value]) -> JsResult<Option<usize>> {
    let position = vm.coerce_integer(&arg(args, 0))?;
    Ok((position >= 0.0 && position < units.len() as f64).then_some(position as usize))
}

fn string_char_at(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "charAt")?;
    let index = unit_index(vm, &units, args)?;
    Ok(string_value(
        index.map(|i| vec![units[i]]).unwrap_or_default(),
    ))
}

fn string_char_code_at(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "charCodeAt")?;
    let index = unit_index(vm, &units, args)?;
    Ok(Value::Number(
        index.map_or(f64::NAN, |i| f64::from(units[i])),
    ))
}

fn string_code_point_at(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "codePointAt")?;
    let Some(index) = unit_index(vm, &units, args)? else {
        return Ok(Value::Undefined);
    };
    let code_point = match code_point_size(&units, index) {
        2 => {
            0x10000
                + ((u32::from(units[index]) - 0xD800) << 10)
                + (u32::from(units[index + 1]) - 0xDC00)
        }
        _ => u32::from(units[index]),
    };
    Ok(Value::Number(f64::from(code_point)))
}

fn string_concat(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let mut units = this_units(vm, this, "concat")?;
    for value in args {
        units.extend(vm.coerce_string(value)?.to_utf16());
    }
    Ok(string_value(units))
}

fn string_index_of(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "indexOf")?;
    let search = vm.coerce_string(&arg(args, 0))?.to_utf16();
    let from = position_arg(vm, args, 1, units.len())?;
    Ok(Value::Number(
        index_of(&units, &search, from).map_or(-1.0, |i| i as f64),
    ))
}

fn string_last_index_of(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "lastIndexOf")?;
    let search = vm.coerce_string(&arg(args, 0))?.to_utf16();
    let position = vm.coerce_number(&arg(args, 1))?;
    let position = if position.is_nan() {
        f64::INFINITY
    } else {
        position.trunc()
    };
    let Some(max_start) = units.len().checked_sub(search.len()) else {
        return Ok(Value::Number(-1.0));
    };
    let start = position.clamp(0.0, max_start as f64) as usize;
    let found = (0..=start).rev().find(|&i| units[i..].starts_with(&search));
    Ok(Value::Number(found.map_or(-1.0, |i| i as f64)))
}

fn string_includes(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "includes")?;
    let search = search_string(vm, &arg(args, 0), "includes")?;
    let from = position_arg(vm, args, 1, units.len())?;
    Ok(Value::Bool(index_of(&units, &search, from).is_some()))
}

fn string_starts_with(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "startsWith")?;
    let search = search_string(vm, &arg(args, 0), "startsWith")?;
    let start = position_arg(vm, args, 1, units.len())?;
    Ok(Value::Bool(units[start..].starts_with(&search)))
}

fn string_ends_with(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "endsWith")?;
    let search = search_string(vm, &arg(args, 0), "endsWith")?;
    let end = match arg(args, 1) {
        Value::Undefined => units.len(),
        _ => position_arg(vm, args, 1, units.len())?,
    };
    Ok(Value::Bool(units[..end].ends_with(&search)))
}

fn string_slice(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "slice")?;
    let length = units.len() as f64;
    let start = relative_index(vm.coerce_integer(&arg(args, 0))?, length);
    let end = match arg(args, 1) {
        Value::Undefined => length,
        value => relative_index(vm.coerce_integer(&value)?, length),
    };
    if start >= end {
        return Ok(string_value(Vec::new()));
    }
    Ok(string_value(units[start as usize..end as usize].to_vec()))
}

fn string_substring(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "substring")?;
    let start = position_arg(vm, args, 0, units.len())?;
    let end = match arg(args, 1) {
        Value::Undefined => units.len(),
        _ => position_arg(vm, args, 1, units.len())?,
    };
    let (from, to) = (start.min(end), start.max(end));
    Ok(string_value(units[from..to].to_vec()))
}

fn string_substr(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "substr")?;
    let size = units.len() as f64;
    let start = relative_index(vm.coerce_integer(&arg(args, 0))?, size);
    let length = match arg(args, 1) {
        Value::Undefined => size,
        value => vm.coerce_integer(&value)?,
    };
    let end = (start + length).min(size);
    if start >= end {
        return Ok(string_value(Vec::new()));
    }
    Ok(string_value(units[start as usize..end as usize].to_vec()))
}

fn string_repeat(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "repeat")?;
    let count = vm.coerce_integer(&arg(args, 0))?;
    if !(0.0..f64::INFINITY).contains(&count) {
        return vm.throw_range("Invalid count value");
    }
    if units.is_empty() || count == 0.0 {
        return Ok(string_value(Vec::new()));
    }
    if count * units.len() as f64 > f64::from(u32::MAX >> 2) {
        return vm.throw_range("Invalid string length");
    }
    vm.reserve_heap(count as usize * units.len() * 2)?;
    Ok(string_value(units.repeat(count as usize)))
}

fn pad(vm: &mut Interpreter, this: &Value, args: &[Value], at_start: bool) -> JsResult<Value> {
    let method = if at_start { "padStart" } else { "padEnd" };
    let units = this_units(vm, this, method)?;
    let max_length = vm.coerce_length(&arg(args, 0))?;
    if max_length <= units.len() as f64 {
        return Ok(string_value(units));
    }
    let filler = match arg(args, 1) {
        Value::Undefined => vec![u16::from(b' ')],
        value => vm.coerce_string(&value)?.to_utf16(),
    };
    if filler.is_empty() {
        return Ok(string_value(units));
    }
    if max_length > f64::from(u32::MAX >> 2) {
        return vm.throw_range("Invalid string length");
    }
    vm.reserve_heap(max_length as usize * 2)?;
    let fill_length = max_length as usize - units.len();
    let fill: Vec<u16> = filler.iter().copied().cycle().take(fill_length).collect();
    let (mut head, tail) = if at_start {
        (fill, units)
    } else {
        (units, fill)
    };
    head.extend(tail);
    Ok(string_value(head))
}

fn string_pad_start(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    pad(vm, this, args, true)
}

fn string_pad_end(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    pad(vm, this, args, false)
}

/// Code units with every lone surrogate replaced by U+FFFD.
pub(crate) fn well_formed(units: &[u16]) -> Vec<u16> {
    char::decode_utf16(units.iter().copied())
        .flat_map(|c| {
            let mut buffer = [0u16; 2];
            c.unwrap_or('\u{FFFD}').encode_utf16(&mut buffer).to_vec()
        })
        .collect()
}

fn string_is_well_formed(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "isWellFormed")?;
    Ok(Value::Bool(char::decode_utf16(units).all(|c| c.is_ok())))
}

fn string_to_well_formed(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let units = this_units(vm, this, "toWellFormed")?;
    Ok(string_value(well_formed(&units)))
}

fn string_to_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::String(
        vm.this_string_value(this, "String.prototype.toString")?,
    ))
}

fn string_value_of(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::String(
        vm.this_string_value(this, "String.prototype.valueOf")?,
    ))
}

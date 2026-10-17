//! URI built-ins (`encodeURI` family, ECMA-262 §19.2.6) and Annex B `escape`/`unescape`.

use super::{Interpreter, JsResult, arg};
use crate::{JsStr, Value};

use super::super::ErrorKind;

const URI_MARKS: &str = "-_.!~*'()";
const URI_RESERVED: &str = ";/?:@&=+$,#";
const ESCAPE_KEPT: &str = "@*_+-./";
const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let global = vm.state.intrinsics.global;
    vm.method(global, "decodeURI", 1, decode_uri)?;
    vm.method(global, "decodeURIComponent", 1, decode_uri_component)?;
    vm.method(global, "encodeURI", 1, encode_uri)?;
    vm.method(global, "encodeURIComponent", 1, encode_uri_component)?;
    vm.method(global, "escape", 1, escape)?;
    vm.method(global, "unescape", 1, unescape)
}

fn is_unreserved(unit: u16) -> bool {
    u8::try_from(unit).is_ok_and(|b| b.is_ascii_alphanumeric() || URI_MARKS.as_bytes().contains(&b))
}

fn is_reserved(unit: u16) -> bool {
    u8::try_from(unit).is_ok_and(|b| URI_RESERVED.as_bytes().contains(&b))
}

fn keep_for_uri(unit: u16) -> bool {
    is_unreserved(unit) || is_reserved(unit)
}

fn never(_: u16) -> bool {
    false
}

fn encode_uri(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let s = vm.to_js_string(&arg(args, 0))?;
    encode(vm, &s, keep_for_uri)
}

fn encode_uri_component(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let s = vm.to_js_string(&arg(args, 0))?;
    encode(vm, &s, is_unreserved)
}

fn decode_uri(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let s = vm.to_js_string(&arg(args, 0))?;
    decode(vm, &s, is_reserved)
}

fn decode_uri_component(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let s = vm.to_js_string(&arg(args, 0))?;
    decode(vm, &s, never)
}

fn push_percent(out: &mut Vec<u16>, byte: u8) {
    out.extend([
        u16::from(b'%'),
        u16::from(HEX_UPPER[usize::from(byte >> 4)]),
        u16::from(HEX_UPPER[usize::from(byte & 15)]),
    ]);
}

fn encode(vm: &mut Interpreter, s: &JsStr, keep: fn(u16) -> bool) -> JsResult<Value> {
    let units = s.to_utf16();
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let unit = units[i];
        i += 1;
        if keep(unit) {
            out.push(unit);
            continue;
        }
        let code = match unit {
            0xD800..=0xDBFF => match units.get(i) {
                Some(&low @ 0xDC00..=0xDFFF) => {
                    i += 1;
                    0x10000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00)
                }
                _ => return Err(vm.error(ErrorKind::Uri, "URI malformed")),
            },
            0xDC00..=0xDFFF => return Err(vm.error(ErrorKind::Uri, "URI malformed")),
            _ => u32::from(unit),
        };
        let ch = char::from_u32(code).expect("surrogates handled above");
        let mut buf = [0u8; 4];
        for &byte in ch.encode_utf8(&mut buf).as_bytes() {
            push_percent(&mut out, byte);
        }
    }
    Ok(Value::String(JsStr::from_utf16(out)))
}

fn hex_value(unit: u16) -> Option<u8> {
    char::from_u32(u32::from(unit))?
        .to_digit(16)
        .map(|d| d as u8)
}

/// Reads the `%XX` escape starting at `at`, if well formed.
fn escape_byte(units: &[u16], at: usize) -> Option<u8> {
    if units.get(at) != Some(&u16::from(b'%')) {
        return None;
    }
    let hi = hex_value(*units.get(at + 1)?)?;
    let lo = hex_value(*units.get(at + 2)?)?;
    Some(hi << 4 | lo)
}

fn decode(vm: &mut Interpreter, s: &JsStr, preserve: fn(u16) -> bool) -> JsResult<Value> {
    let units = s.to_utf16();
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        if units[i] != u16::from(b'%') {
            out.push(units[i]);
            i += 1;
            continue;
        }
        let Some(first) = escape_byte(&units, i) else {
            return Err(vm.error(ErrorKind::Uri, "URI malformed"));
        };
        if first < 0x80 {
            if preserve(u16::from(first)) {
                out.extend_from_slice(&units[i..i + 3]);
            } else {
                out.push(u16::from(first));
            }
            i += 3;
            continue;
        }
        let Some((code, consumed)) = decode_multibyte(&units, i, first) else {
            return Err(vm.error(ErrorKind::Uri, "URI malformed"));
        };
        let ch = char::from_u32(code).expect("validated scalar value");
        out.extend(ch.encode_utf16(&mut [0u16; 2]).iter());
        i += consumed;
    }
    Ok(Value::String(JsStr::from_utf16(out)))
}

/// Decodes a UTF-8 sequence of `%XX` escapes; rejects overlong, surrogate and out-of-range forms.
fn decode_multibyte(units: &[u16], at: usize, first: u8) -> Option<(u32, usize)> {
    let (count, min, lead_bits) = match first {
        0xC0..=0xDF => (2, 0x80, first & 0x1F),
        0xE0..=0xEF => (3, 0x800, first & 0x0F),
        0xF0..=0xF7 => (4, 0x10000, first & 0x07),
        _ => return None,
    };
    let mut code = u32::from(lead_bits);
    for k in 1..count {
        let byte = escape_byte(units, at + 3 * k)?;
        if byte & 0xC0 != 0x80 {
            return None;
        }
        code = code << 6 | u32::from(byte & 0x3F);
    }
    let valid = code >= min && char::from_u32(code).is_some();
    valid.then_some((code, 3 * count))
}

fn escape(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let s = vm.to_js_string(&arg(args, 0))?;
    let units = s.to_utf16();
    let mut out = Vec::with_capacity(units.len());
    for unit in units {
        let ascii = u8::try_from(unit).ok();
        match ascii {
            Some(b) if b.is_ascii_alphanumeric() || ESCAPE_KEPT.as_bytes().contains(&b) => {
                out.push(unit)
            }
            Some(b) => push_percent(&mut out, b),
            None => {
                out.extend([u16::from(b'%'), u16::from(b'u')]);
                out.extend(
                    (0..4)
                        .rev()
                        .map(|n| u16::from(HEX_UPPER[usize::from((unit >> (4 * n)) & 15)])),
                );
            }
        }
    }
    Ok(Value::String(JsStr::from_utf16(out)))
}

/// Parses `count` hex digits at `at`.
fn hex_run(units: &[u16], at: usize, count: usize) -> Option<u16> {
    let digits = units.get(at..at + count)?;
    digits
        .iter()
        .try_fold(0u16, |acc, &d| Some(acc << 4 | u16::from(hex_value(d)?)))
}

fn unescape(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let s = vm.to_js_string(&arg(args, 0))?;
    let units = s.to_utf16();
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let percent = units[i] == u16::from(b'%');
        let wide = percent && units.get(i + 1) == Some(&u16::from(b'u'));
        let decoded = match (percent, wide) {
            (false, _) => None,
            (true, true) => hex_run(&units, i + 2, 4).map(|v| (v, 6)),
            (true, false) => hex_run(&units, i + 1, 2).map(|v| (v, 3)),
        };
        match decoded {
            Some((value, consumed)) => {
                out.push(value);
                i += consumed;
            }
            None => {
                out.push(units[i]);
                i += 1;
            }
        }
    }
    Ok(Value::String(JsStr::from_utf16(out)))
}

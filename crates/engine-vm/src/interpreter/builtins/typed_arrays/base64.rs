//! `Uint8Array` base64 and hex conversions (proposal-arraybuffer-base64): the codecs are
//! pure functions over code units and bytes; the natives below only do the spec plumbing.

use std::rc::Rc;

use super::super::string::string_value;
use super::super::{Builtin, ErrorKind, Interpreter, Intrinsic, JsResult, arg, object_value};
use super::{ElementKind, TypedArray};
use crate::convert::to_boolean;
use crate::{ObjectRef, Value};

const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const ALPHABETS: [&str; 2] = ["base64", "base64url"];
const LAST_CHUNKS: [&str; 3] = ["loose", "strict", "stop-before-partial"];

#[derive(Clone, Copy, PartialEq)]
enum Alphabet {
    Standard,
    Url,
}

#[derive(Clone, Copy, PartialEq)]
enum LastChunk {
    Loose,
    Strict,
    StopBeforePartial,
}

/// What a decoder produced: the bytes, the code units consumed and the SyntaxError (if
/// any) to raise after the bytes were written.
struct Decoded {
    bytes: Vec<u8>,
    read: usize,
    error: Option<&'static str>,
}

const BAD_BASE64: &str = "The base64 input is not valid";
const BAD_HEX: &str = "The hex input is not valid";

fn is_ascii_whitespace(unit: u16) -> bool {
    matches!(unit, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

fn skip_whitespace(units: &[u16], mut index: usize) -> usize {
    while units.get(index).is_some_and(|&u| is_ascii_whitespace(u)) {
        index += 1;
    }
    index
}

fn sextet(unit: u16, alphabet: Alphabet) -> Option<u8> {
    let table = match alphabet {
        Alphabet::Standard => STANDARD,
        Alphabet::Url => URL,
    };
    let byte = u8::try_from(unit).ok()?;
    table.iter().position(|&c| c == byte).map(|i| i as u8)
}

/// DecodeBase64Chunk: appends the bytes of 2 to 4 sextets; `false` when `strict` and the
/// unused trailing bits are not zero.
fn decode_chunk(sextets: &[u8], strict: bool, out: &mut Vec<u8>) -> bool {
    let mut padded = [0u8; 4];
    padded[..sextets.len()].copy_from_slice(sextets);
    let bits = padded
        .iter()
        .fold(0u32, |acc, &s| (acc << 6) | u32::from(s));
    let bytes = [(bits >> 16) as u8, (bits >> 8) as u8, bits as u8];
    let keep = sextets.len() - 1;
    if strict && sextets.len() < 4 && bytes[keep] != 0 {
        return false;
    }
    out.extend_from_slice(&bytes[..keep]);
    true
}

/// FromBase64, stopping once `max_len` bytes were produced.
fn decode_base64(
    units: &[u16],
    alphabet: Alphabet,
    last: LastChunk,
    max_len: Option<usize>,
) -> Decoded {
    let max_len = max_len.unwrap_or(usize::MAX);
    let mut out = Decoded {
        bytes: Vec::new(),
        read: 0,
        error: None,
    };
    if max_len == 0 {
        return out;
    }
    let fail = |mut out: Decoded| {
        out.error = Some(BAD_BASE64);
        out
    };
    let mut chunk: Vec<u8> = Vec::with_capacity(4);
    let mut index = 0;
    loop {
        index = skip_whitespace(units, index);
        if index == units.len() {
            if chunk.is_empty() {
                out.read = units.len();
                return out;
            }
            if last == LastChunk::StopBeforePartial {
                return out;
            }
            if last == LastChunk::Strict || chunk.len() == 1 {
                return fail(out);
            }
            decode_chunk(&chunk, false, &mut out.bytes);
            out.read = units.len();
            return out;
        }
        let unit = units[index];
        index += 1;
        if unit == u16::from(b'=') {
            return decode_padding(units, index, &chunk, last, out);
        }
        let Some(value) = sextet(unit, alphabet) else {
            return fail(out);
        };
        let remaining = max_len - out.bytes.len();
        if (remaining == 1 && chunk.len() == 2) || (remaining == 2 && chunk.len() == 3) {
            return out;
        }
        chunk.push(value);
        if chunk.len() == 4 {
            decode_chunk(&chunk, false, &mut out.bytes);
            chunk.clear();
            out.read = index;
            if out.bytes.len() == max_len {
                return out;
            }
        }
    }
}

/// The `=` branch of FromBase64: `index` is just past the first `=`.
fn decode_padding(
    units: &[u16],
    mut index: usize,
    chunk: &[u8],
    last: LastChunk,
    mut out: Decoded,
) -> Decoded {
    let fail = |mut out: Decoded| {
        out.error = Some(BAD_BASE64);
        out
    };
    if chunk.len() < 2 {
        return fail(out);
    }
    index = skip_whitespace(units, index);
    if chunk.len() == 2 {
        if index == units.len() {
            return if last == LastChunk::StopBeforePartial {
                out
            } else {
                fail(out)
            };
        }
        if units[index] == u16::from(b'=') {
            index = skip_whitespace(units, index + 1);
        }
    }
    if index < units.len() || !decode_chunk(chunk, last == LastChunk::Strict, &mut out.bytes) {
        return fail(out);
    }
    out.read = units.len();
    out
}

fn hexit(unit: u16) -> Option<u8> {
    char::from_u32(u32::from(unit))?
        .to_digit(16)
        .map(|d| d as u8)
}

/// FromHex, stopping once `max_len` bytes were produced.
fn decode_hex(units: &[u16], max_len: Option<usize>) -> Decoded {
    let max_len = max_len.unwrap_or(usize::MAX);
    let mut out = Decoded {
        bytes: Vec::new(),
        read: 0,
        error: None,
    };
    if !units.len().is_multiple_of(2) {
        out.error = Some(BAD_HEX);
        return out;
    }
    while out.read < units.len() && out.bytes.len() < max_len {
        match (hexit(units[out.read]), hexit(units[out.read + 1])) {
            (Some(high), Some(low)) => out.bytes.push(high << 4 | low),
            _ => {
                out.error = Some(BAD_HEX);
                return out;
            }
        }
        out.read += 2;
    }
    out
}

fn encode_base64(bytes: &[u8], alphabet: Alphabet, omit_padding: bool) -> String {
    let table = match alphabet {
        Alphabet::Standard => STANDARD,
        Alphabet::Url => URL,
    };
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &b)| acc | u32::from(b) << (16 - 8 * i));
        for slot in 0..=chunk.len() {
            text.push(char::from(table[(bits >> (18 - 6 * slot) & 63) as usize]));
        }
        if !omit_padding {
            text.extend(std::iter::repeat_n('=', 3 - chunk.len()));
        }
    }
    text
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let constructor = vm.intrinsic(Intrinsic::Uint8Array);
    vm.method(constructor, "fromBase64", 1, from_base64)?;
    vm.method(constructor, "fromHex", 1, from_hex)?;
    let proto = vm.get_named(constructor, "prototype")?;
    let proto = vm
        .as_object(&proto)
        .expect("constructor prototypes are objects");
    let methods: [(&str, u32, Builtin); 4] = [
        ("toBase64", 0, to_base64),
        ("toHex", 0, to_hex),
        ("setFromBase64", 1, set_from_base64),
        ("setFromHex", 1, set_from_hex),
    ];
    for (name, length, native) in methods {
        vm.method(proto, name, length, native)?;
    }
    Ok(())
}

/// ValidateUint8Array: a `Uint8Array` (not clamped, not another element type).
fn uint8_this(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<Rc<TypedArray>> {
    match vm.typed_array_of(this) {
        Some(array) if array.kind == ElementKind::Uint8 => Ok(array),
        _ => vm.throw_type(&format!(
            "Uint8Array.prototype.{method}: this is not a Uint8Array"
        )),
    }
}

fn string_arg(vm: &mut Interpreter, value: &Value) -> JsResult<Vec<u16>> {
    match value {
        Value::String(text) => Ok(text.to_utf16()),
        _ => vm.throw_type("The argument must be a string"),
    }
}

/// GetOptionsObject.
fn options_object(vm: &mut Interpreter, value: &Value) -> JsResult<Option<ObjectRef>> {
    match value {
        Value::Undefined => Ok(None),
        _ => match vm.as_object(value) {
            Some(object) => Ok(Some(object)),
            None => vm.throw_type("The options argument must be an object"),
        },
    }
}

/// Reads `name` and returns the index of its value in `allowed` (0 when absent); the
/// value is never coerced, so anything but one of the strings is a TypeError.
fn read_choice(
    vm: &mut Interpreter,
    options: Option<ObjectRef>,
    name: &str,
    allowed: &[&str],
) -> JsResult<usize> {
    let Some(options) = options else { return Ok(0) };
    match vm.get_named(options, name)? {
        Value::Undefined => Ok(0),
        Value::String(text) => match allowed.iter().position(|a| text.to_string_lossy() == *a) {
            Some(index) => Ok(index),
            None => vm.throw_type(&format!("Invalid value for the {name} option")),
        },
        _ => vm.throw_type(&format!("Invalid value for the {name} option")),
    }
}

fn read_alphabet(vm: &mut Interpreter, options: Option<ObjectRef>) -> JsResult<Alphabet> {
    let index = read_choice(vm, options, "alphabet", &ALPHABETS)?;
    Ok([Alphabet::Standard, Alphabet::Url][index])
}

fn read_last_chunk(vm: &mut Interpreter, options: Option<ObjectRef>) -> JsResult<LastChunk> {
    let index = read_choice(vm, options, "lastChunkHandling", &LAST_CHUNKS)?;
    Ok([
        LastChunk::Loose,
        LastChunk::Strict,
        LastChunk::StopBeforePartial,
    ][index])
}

/// GetUint8ArrayBytes: a copy of the viewed bytes; TypeError when detached or out of bounds.
fn bytes_of(vm: &mut Interpreter, array: &TypedArray) -> JsResult<Vec<u8>> {
    let Some(length) = array.length() else {
        return vm.throw_type("The typed array is detached or out of bounds");
    };
    let start = array.view.byte_offset;
    let copy = |bytes: &mut [u8]| bytes.get(start..start + length).map(<[u8]>::to_vec);
    match array.view.store.with_bytes(copy) {
        Some(bytes) => Ok(bytes),
        None => vm.throw_type("The typed array is detached or out of bounds"),
    }
}

fn text_value(text: &str) -> Value {
    string_value(text.encode_utf16().collect())
}

/// A new Uint8Array holding `bytes`.
fn new_uint8_array(vm: &mut Interpreter, bytes: &[u8]) -> JsResult<Value> {
    let constructor = vm.intrinsic(Intrinsic::Uint8Array);
    let proto = vm.get_named(constructor, "prototype")?;
    let proto = vm
        .as_object(&proto)
        .expect("constructor prototypes are objects");
    let buffer_proto = vm.intrinsic(Intrinsic::ArrayBufferPrototype);
    let buffer = vm.create_buffer(buffer_proto, bytes.len(), None, false)?;
    let array = vm.create_typed_array(proto, ElementKind::Uint8, buffer, 0, Some(bytes.len()))?;
    let typed = vm
        .typed_array_of(&object_value(array))
        .expect("fresh typed array");
    write_bytes(&typed, bytes);
    Ok(object_value(array))
}

fn write_bytes(array: &TypedArray, bytes: &[u8]) {
    let start = array.view.byte_offset;
    array.view.store.with_bytes(|dst| {
        if let Some(window) = dst.get_mut(start..start + bytes.len()) {
            window.copy_from_slice(bytes);
        }
    });
}

fn syntax_error<T>(vm: &mut Interpreter, message: &str) -> JsResult<T> {
    Err(vm.error(ErrorKind::Syntax, message))
}

fn from_base64(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let units = string_arg(vm, &arg(args, 0))?;
    let options = options_object(vm, &arg(args, 1))?;
    let alphabet = read_alphabet(vm, options)?;
    let last = read_last_chunk(vm, options)?;
    let decoded = decode_base64(&units, alphabet, last, None);
    match decoded.error {
        Some(message) => syntax_error(vm, message),
        None => new_uint8_array(vm, &decoded.bytes),
    }
}

fn from_hex(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let units = string_arg(vm, &arg(args, 0))?;
    let decoded = decode_hex(&units, None);
    match decoded.error {
        Some(message) => syntax_error(vm, message),
        None => new_uint8_array(vm, &decoded.bytes),
    }
}

fn to_base64(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let array = uint8_this(vm, this, "toBase64")?;
    let options = options_object(vm, &arg(args, 0))?;
    let alphabet = read_alphabet(vm, options)?;
    let omit_padding = match options {
        Some(options) => to_boolean(&vm.get_named(options, "omitPadding")?),
        None => false,
    };
    let bytes = bytes_of(vm, &array)?;
    Ok(text_value(&encode_base64(&bytes, alphabet, omit_padding)))
}

fn to_hex(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let array = uint8_this(vm, this, "toHex")?;
    let bytes = bytes_of(vm, &array)?;
    Ok(text_value(&encode_hex(&bytes)))
}

/// Writes what was decoded (even when it ended in an error), then raises the error or
/// returns `{ read, written }`.
fn finish_set(vm: &mut Interpreter, array: &TypedArray, decoded: Decoded) -> JsResult<Value> {
    write_bytes(array, &decoded.bytes);
    if let Some(message) = decoded.error {
        return syntax_error(vm, message);
    }
    let result = vm.new_object()?;
    let fields = [("read", decoded.read), ("written", decoded.bytes.len())];
    for (name, count) in fields {
        let key = vm.realm.key_str(name);
        vm.create_data_property(result, key, Value::Number(count as f64))?;
    }
    Ok(object_value(result))
}

fn set_from_base64(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let array = uint8_this(vm, this, "setFromBase64")?;
    let units = string_arg(vm, &arg(args, 0))?;
    let options = options_object(vm, &arg(args, 1))?;
    let alphabet = read_alphabet(vm, options)?;
    let last = read_last_chunk(vm, options)?;
    let Some(capacity) = array.length() else {
        return vm.throw_type("The typed array is detached or out of bounds");
    };
    let decoded = decode_base64(&units, alphabet, last, Some(capacity));
    finish_set(vm, &array, decoded)
}

fn set_from_hex(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let array = uint8_this(vm, this, "setFromHex")?;
    let units = string_arg(vm, &arg(args, 0))?;
    let Some(capacity) = array.length() else {
        return vm.throw_type("The typed array is detached or out of bounds");
    };
    let decoded = decode_hex(&units, Some(capacity));
    finish_set(vm, &array, decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn decode(text: &str, last: LastChunk, max: Option<usize>) -> Decoded {
        decode_base64(&units(text), Alphabet::Standard, last, max)
    }

    #[test]
    fn decodes_full_and_partial_chunks() {
        assert_eq!(decode("SGVsbG8=", LastChunk::Strict, None).bytes, b"Hello");
        assert_eq!(decode("SGVsbG8", LastChunk::Loose, None).bytes, b"Hello");
        assert!(decode("SGVsbG8", LastChunk::Strict, None).error.is_some());
        let stopped = decode("SGVsbG8", LastChunk::StopBeforePartial, None);
        assert_eq!((stopped.bytes.as_slice(), stopped.read), (&b"Hel"[..], 4));
    }

    #[test]
    fn capacity_stops_before_a_chunk_that_does_not_fit() {
        let out = decode("SGVsbG8=", LastChunk::Loose, Some(4));
        assert_eq!((out.bytes.as_slice(), out.read), (&b"Hel"[..], 4));
        let out = decode("SGVsbG8=", LastChunk::Loose, Some(3));
        assert_eq!((out.bytes.as_slice(), out.read), (&b"Hel"[..], 4));
    }

    #[test]
    fn encodes_with_and_without_padding() {
        assert_eq!(encode_base64(&[72, 101], Alphabet::Standard, false), "SGU=");
        assert_eq!(encode_base64(&[72, 101], Alphabet::Standard, true), "SGU");
        assert_eq!(encode_base64(&[251, 255], Alphabet::Url, false), "-_8=");
        assert_eq!(encode_hex(&[0, 255, 16]), "00ff10");
    }

    #[test]
    fn hex_rejects_odd_length_and_non_hex() {
        assert!(decode_hex(&units("abc"), None).error.is_some());
        let out = decode_hex(&units("48zz"), None);
        assert_eq!(
            (out.bytes.as_slice(), out.read, out.error.is_some()),
            (&[0x48][..], 2, true)
        );
        assert_eq!(decode_hex(&units("48656c"), Some(2)).bytes, [0x48, 0x65]);
    }
}

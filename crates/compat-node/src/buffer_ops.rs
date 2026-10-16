//! Byte/string transcoding for `Buffer` and `StringDecoder`. The JS side passes an encoding name
//! and gets bytes back as an `ArrayBuffer`, or hands a typed-array window and gets a string.

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, JsString, Value};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Clone, Copy)]
enum Encoding {
    Utf8,
    Utf16le,
    Latin1,
    Ascii,
    Base64,
    Base64Url,
    Hex,
}

impl Encoding {
    /// Canonical names only: the JS side normalizes aliases.
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "utf8" => Self::Utf8,
            "utf16le" => Self::Utf16le,
            "latin1" => Self::Latin1,
            "ascii" => Self::Ascii,
            "base64" => Self::Base64,
            "base64url" => Self::Base64Url,
            "hex" => Self::Hex,
            _ => return None,
        })
    }
}

/// The ops (Buffer bytes, `process`, `setImmediate`), registered by the host before
/// `install_compat_node`.
pub fn compat_node_host_functions() -> Vec<HostFunctionDef> {
    let mut defs = vec![
        HostFunctionDef {
            name: "op_buffer_encode",
            arity: 2,
            callback: op_buffer_encode,
        },
        HostFunctionDef {
            name: "op_buffer_decode",
            arity: 2,
            callback: op_buffer_decode,
        },
        HostFunctionDef {
            name: "op_buffer_byte_length",
            arity: 2,
            callback: op_buffer_byte_length,
        },
    ];
    defs.extend(crate::process_ops::process_host_functions());
    defs.extend(crate::crypto_ops::crypto_host_functions());
    defs.extend(crate::fs_ops::fs_host_functions());
    defs
}

fn encoding_arg(value: Option<&Value>) -> Result<Encoding, HostError> {
    match value {
        Some(Value::String(name)) => Encoding::parse(&name.to_string_lossy())
            .ok_or_else(|| HostError::TypeError("unknown buffer encoding".into())),
        _ => Err(HostError::TypeError("encoding must be a string".into())),
    }
}

/// `op_buffer_encode(text, encoding) -> ArrayBuffer`
fn op_buffer_encode(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (Some(Value::String(text)), encoding) = (args.first(), encoding_arg(args.get(1))?) else {
        return Err(HostError::TypeError(
            "op_buffer_encode expects (text: string, encoding: string)".into(),
        ));
    };
    Ok(ctx.create_array_buffer(&encode(text.as_utf16(), encoding)))
}

/// `op_buffer_byte_length(text, encoding) -> number`
fn op_buffer_byte_length(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (Some(Value::String(text)), encoding) = (args.first(), encoding_arg(args.get(1))?) else {
        return Err(HostError::TypeError(
            "op_buffer_byte_length expects (text: string, encoding: string)".into(),
        ));
    };
    // shortcut: encodes to count; upgrade to closed-form lengths if this shows up in profiles.
    Ok(Value::Number(encode(text.as_utf16(), encoding).len() as f64))
}

/// `op_buffer_decode(bytes, encoding) -> string`; `bytes` is a typed array (its window) or buffer.
fn op_buffer_decode(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let encoding = encoding_arg(args.get(1))?;
    let bytes = args
        .first()
        .and_then(|value| ctx.read_bytes(value))
        .ok_or_else(|| HostError::TypeError("op_buffer_decode expects a buffer".into()))?;
    Ok(ctx.create_string(JsString::from_utf16(decode(&bytes, encoding))))
}

fn encode(units: &[u16], encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => char::decode_utf16(units.iter().copied())
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect::<String>()
            .into_bytes(),
        Encoding::Utf16le => units.iter().flat_map(|unit| unit.to_le_bytes()).collect(),
        Encoding::Latin1 | Encoding::Ascii => units.iter().map(|&unit| unit as u8).collect(),
        Encoding::Base64 | Encoding::Base64Url => decode_base64(units),
        Encoding::Hex => decode_hex(units),
    }
}

fn decode(bytes: &[u8], encoding: Encoding) -> Vec<u16> {
    match encoding {
        Encoding::Utf8 => String::from_utf8_lossy(bytes).encode_utf16().collect(),
        Encoding::Utf16le => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect(),
        Encoding::Latin1 => bytes.iter().map(|&byte| u16::from(byte)).collect(),
        Encoding::Ascii => bytes.iter().map(|&byte| u16::from(byte & 0x7f)).collect(),
        Encoding::Base64 => encode_base64(bytes, B64, true),
        Encoding::Base64Url => encode_base64(bytes, B64_URL, false),
        Encoding::Hex => {
            const DIGITS: &[u8; 16] = b"0123456789abcdef";
            bytes
                .iter()
                .flat_map(|&byte| {
                    [
                        u16::from(DIGITS[usize::from(byte >> 4)]),
                        u16::from(DIGITS[usize::from(byte & 15)]),
                    ]
                })
                .collect()
        }
    }
}

fn encode_base64(bytes: &[u8], alphabet: &[u8; 64], pad: bool) -> Vec<u16> {
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().enumerate().fold(0u32, |acc, (i, &byte)| {
            acc | u32::from(byte) << (16 - 8 * i)
        });
        for i in 0..=chunk.len() {
            out.push(u16::from(alphabet[(group >> (18 - 6 * i) & 63) as usize]));
        }
        if pad {
            out.extend(std::iter::repeat_n(u16::from(b'='), 3 - chunk.len()));
        }
    }
    out
}

/// Lenient like Node: both alphabets, characters outside them are skipped, `=` ends the input.
fn decode_base64(units: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &unit in units {
        let sextet = match u8::try_from(unit) {
            Ok(b'A'..=b'Z') => unit - u16::from(b'A'),
            Ok(b'a'..=b'z') => unit - u16::from(b'a') + 26,
            Ok(b'0'..=b'9') => unit - u16::from(b'0') + 52,
            Ok(b'+' | b'-') => 62,
            Ok(b'/' | b'_') => 63,
            Ok(b'=') => break,
            _ => continue,
        };
        acc = acc << 6 | u32::from(sextet);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// Stops at the first character that is not a hex digit pair, like Node.
fn decode_hex(units: &[u16]) -> Vec<u8> {
    let digit = |unit: u16| char::from_u32(u32::from(unit)).and_then(|c| c.to_digit(16));
    units
        .as_chunks::<2>()
        .0
        .iter()
        .map_while(|pair| Some((digit(pair[0])? << 4 | digit(pair[1])?) as u8))
        .collect()
}

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
        HostFunctionDef {
            name: "op_buffer_write",
            arity: 5,
            callback: op_buffer_write,
        },
    ];
    defs.extend(crate::async_context_ops::async_context_host_functions());
    defs.extend(crate::process_ops::process_host_functions());
    defs.extend(crate::crypto_ops::crypto_host_functions());
    defs.extend(crate::fs_ops::fs_host_functions());
    defs.extend(crate::os_ops::os_host_functions());
    defs.extend(crate::v8_ops::v8_host_functions());
    defs.extend(crate::zlib_ops::zlib_host_functions());
    defs.extend(crate::net_ops::net_node_host_functions());
    defs.extend(crate::child_process_ops::child_process_host_functions());
    defs.extend(crate::worker_threads_ops::worker_threads_host_functions());
    defs.extend(crate::dns_query::dns_query_host_functions());
    defs.extend(crate::http_ops::http_host_functions());
    defs.extend(crate::http_client_ops::http_client_host_functions());
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
    Ok(ctx.create_array_buffer(&encode_string(text, encoding)))
}

/// `op_buffer_byte_length(text, encoding) -> number`
fn op_buffer_byte_length(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (Some(Value::String(text)), encoding) = (args.first(), encoding_arg(args.get(1))?) else {
        return Err(HostError::TypeError(
            "op_buffer_byte_length expects (text: string, encoding: string)".into(),
        ));
    };
    let length = match encoding {
        Encoding::Utf8 => utf8_length(text),
        Encoding::Utf16le => text.len() * 2,
        Encoding::Latin1 | Encoding::Ascii => text.len(),
        // shortcut: encodes to count; closed forms for the lenient decoders when profiles ask.
        Encoding::Base64 | Encoding::Base64Url | Encoding::Hex => {
            encode_string(text, encoding).len()
        }
    };
    Ok(Value::Number(length as f64))
}

/// `op_buffer_write(target, text, encoding, offset, length) -> number`: bytes written, or -1 when
/// the adapter cannot borrow `target` (the JS side then takes the copy path). Never splits a
/// UTF-8 sequence or a UTF-16 unit. `offset` and `length` are validated by the caller.
fn op_buffer_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (
        Some(target),
        Some(Value::String(text)),
        Some(Value::Number(offset)),
        Some(Value::Number(max)),
    ) = (args.first(), args.get(1), args.get(3), args.get(4))
    else {
        return Err(HostError::TypeError(
            "op_buffer_write expects (target, text, encoding, offset, length)".into(),
        ));
    };
    let encoding = encoding_arg(args.get(2))?;
    let bytes = encode_string(text, encoding);
    let mut written = None;
    let borrowed = ctx.with_bytes_mut(target, &mut |window| {
        let start = (*offset as usize).min(window.len());
        let mut count = bytes.len().min(*max as usize).min(window.len() - start);
        if count < bytes.len() {
            match encoding {
                Encoding::Utf8 => {
                    while count > 0 && bytes[count] & 0xc0 == 0x80 {
                        count -= 1;
                    }
                }
                Encoding::Utf16le => count &= !1,
                _ => {}
            }
        }
        window[start..start + count].copy_from_slice(&bytes[..count]);
        written = Some(count);
    });
    Ok(Value::Number(
        written.filter(|_| borrowed).map_or(-1.0, |n| n as f64),
    ))
}

/// `op_buffer_decode(bytes, encoding, start?, end?) -> string`; `bytes` is a typed array or buffer
/// and `start..end` (clamped) selects a window of it.
fn op_buffer_decode(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let encoding = encoding_arg(args.get(1))?;
    let bound = |index: usize, default: usize| match args.get(index) {
        Some(Value::Number(n)) => *n as usize,
        _ => default,
    };
    let (start, end) = (bound(2, 0), bound(3, usize::MAX));
    let window = |bytes: &[u8]| {
        let end = end.min(bytes.len());
        decode(&bytes[start.min(end)..end], encoding)
    };
    let value = args
        .first()
        .ok_or_else(|| HostError::TypeError("op_buffer_decode expects a buffer".into()))?;
    let mut text = None;
    if !ctx.with_bytes(value, &mut |bytes| text = Some(window(bytes))) {
        let copied = ctx
            .read_bytes(value)
            .ok_or_else(|| HostError::TypeError("op_buffer_decode expects a buffer".into()))?;
        text = Some(window(&copied));
    }
    Ok(ctx.create_string(text.expect("decoded by one of the two paths")))
}

/// UTF-8 length of the string, lone surrogates counting as U+FFFD (3 bytes), no allocation.
fn utf8_length(text: &JsString) -> usize {
    if let Some(bytes) = text.as_latin1() {
        return bytes.len() + bytes.iter().filter(|&&byte| byte >= 0x80).count();
    }
    char::decode_utf16(text.as_utf16().iter().copied())
        .map(|unit| unit.map_or(3, char::len_utf8))
        .sum()
}

/// Encodes with the one-byte fast paths; same bytes as `encode(text.as_utf16(), ..)`.
fn encode_string(text: &JsString, encoding: Encoding) -> Vec<u8> {
    match (text.as_latin1(), encoding) {
        (Some(bytes), Encoding::Latin1 | Encoding::Ascii) => bytes.to_vec(),
        (Some(bytes), Encoding::Utf8) if bytes.is_ascii() => bytes.to_vec(),
        _ => encode(text.as_utf16(), encoding),
    }
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

fn decode(bytes: &[u8], encoding: Encoding) -> JsString {
    match encoding {
        Encoding::Utf8 if bytes.is_ascii() => JsString::from_latin1(bytes.to_vec()),
        Encoding::Utf8 => {
            JsString::from_utf16(String::from_utf8_lossy(bytes).encode_utf16().collect())
        }
        Encoding::Utf16le => JsString::from_utf16(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect(),
        ),
        Encoding::Latin1 => JsString::from_latin1(bytes.to_vec()),
        Encoding::Ascii => JsString::from_latin1(bytes.iter().map(|&byte| byte & 0x7f).collect()),
        Encoding::Base64 => JsString::from_latin1(encode_base64(bytes, B64, true)),
        Encoding::Base64Url => JsString::from_latin1(encode_base64(bytes, B64_URL, false)),
        Encoding::Hex => {
            const DIGITS: &[u8; 16] = b"0123456789abcdef";
            JsString::from_latin1(
                bytes
                    .iter()
                    .flat_map(|&byte| {
                        [
                            DIGITS[usize::from(byte >> 4)],
                            DIGITS[usize::from(byte & 15)],
                        ]
                    })
                    .collect(),
            )
        }
    }
}

fn encode_base64(bytes: &[u8], alphabet: &[u8; 64], pad: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().enumerate().fold(0u32, |acc, (i, &byte)| {
            acc | u32::from(byte) << (16 - 8 * i)
        });
        for i in 0..=chunk.len() {
            out.push(alphabet[(group >> (18 - 6 * i) & 63) as usize]);
        }
        if pad {
            out.extend(std::iter::repeat_n(b'=', 3 - chunk.len()));
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

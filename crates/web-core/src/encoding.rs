use std::collections::HashMap;

use encoding_rs::{CoderResult, Decoder, DecoderResult, Encoding, REPLACEMENT};
use velyr_engine_api::{
    CallContext, HostError, HostFunctionDef, Isolate, JsException, JsString, Source, Value,
};

/// Bootstrap script: defines `TextEncoder`, `TextDecoder`, `atob` and `btoa` on top of the private
/// `op_encoding_*` bindings.
pub const ENCODING_JS: &str = include_str!("encoding.js");

/// Streaming decoders, keyed by the id the JS `TextDecoder` holds between `stream: true` calls.
#[derive(Default)]
struct DecoderTable(HashMap<u32, Decoder>);

pub fn encoding_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_encoding_label",
            arity: 1,
            callback: op_encoding_label,
        },
        HostFunctionDef {
            name: "op_encoding_decode",
            arity: 6,
            callback: op_encoding_decode,
        },
        HostFunctionDef {
            name: "op_encoding_encode",
            arity: 1,
            callback: op_encoding_encode,
        },
        HostFunctionDef {
            name: "op_encoding_encode_into",
            arity: 2,
            callback: op_encoding_encode_into,
        },
    ]
}

/// Registers the decoder table and runs [`ENCODING_JS`]. Call from the `install` hook of
/// `run_file`, after the `encoding_host_functions` are registered.
pub fn install_encoding(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate.set_slot(DecoderTable::default());
    isolate
        .eval_script(&Source {
            name: "velyr:encoding".into(),
            text: ENCODING_JS.into(),
        })
        .map(drop)
}

fn lookup(label: &str) -> Option<&'static Encoding> {
    Encoding::for_label(label.as_bytes()).filter(|&encoding| encoding != REPLACEMENT)
}

/// `op_encoding_label(label) -> name | null`: the lowercase Encoding Standard name, or null for an
/// unknown label (and for `replacement`, which `TextDecoder` rejects).
fn op_encoding_label(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(label)] = args else {
        return Err(HostError::TypeError(
            "op_encoding_label expects (label: string)".into(),
        ));
    };
    Ok(match lookup(&label.to_string_lossy()) {
        Some(encoding) => {
            ctx.create_string(JsString::from_str(&encoding.name().to_ascii_lowercase()))
        }
        None => Value::Null,
    })
}

/// `op_encoding_decode(name, bytes, fatal, ignoreBOM, stream, id) -> string`
///
/// `bytes` is a binary string (one byte per UTF-16 unit). With `stream` the decoder keeps its
/// pending bytes under `id` until a call with `stream: false` flushes it.
fn op_encoding_decode(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [
        Value::String(name),
        Value::String(bytes),
        Value::Bool(fatal),
        Value::Bool(ignore_bom),
        Value::Bool(stream),
        Value::Number(id),
    ] = args
    else {
        return Err(HostError::TypeError(
            "op_encoding_decode expects (name, bytes, fatal, ignoreBOM, stream, id)".into(),
        ));
    };
    let encoding = lookup(&name.to_string_lossy())
        .ok_or_else(|| HostError::TypeError("op_encoding_decode got an unknown encoding".into()))?;
    let bytes: Vec<u8> = bytes.as_utf16().iter().map(|&unit| unit as u8).collect();
    let id = *id as u32;
    let table = ctx
        .isolate_slot::<DecoderTable>()
        .ok_or_else(|| HostError::TypeError("encoding bootstrap is not installed".into()))?;
    let mut decoder = table.0.remove(&id).unwrap_or_else(|| {
        if *ignore_bom {
            encoding.new_decoder_without_bom_handling()
        } else {
            encoding.new_decoder_with_bom_removal()
        }
    });
    let decoded = decode_chunk(&mut decoder, &bytes, *fatal, !*stream);
    if *stream && decoded.is_some() {
        table.0.insert(id, decoder);
    }
    match decoded {
        Some(text) => Ok(ctx.create_string(JsString::from_str(&text))),
        None => Err(ctx.throw_type_error(&format!(
            "The encoded data was not valid for encoding {}",
            encoding.name().to_ascii_lowercase()
        ))),
    }
}

/// Decodes one chunk; `None` means a fatal decoder met malformed input.
fn decode_chunk(decoder: &mut Decoder, bytes: &[u8], fatal: bool, last: bool) -> Option<String> {
    let mut out = String::new();
    let mut offset = 0;
    loop {
        // Never grows the String on its own, so make room for the rest of the input first.
        let room = decoder.max_utf8_buffer_length(bytes.len() - offset)?;
        out.reserve(room);
        let rest = &bytes[offset..];
        if fatal {
            let (result, read) = decoder.decode_to_string_without_replacement(rest, &mut out, last);
            offset += read;
            match result {
                DecoderResult::InputEmpty => return Some(out),
                DecoderResult::OutputFull => {}
                DecoderResult::Malformed(..) => return None,
            }
        } else {
            let (result, read, _) = decoder.decode_to_string(rest, &mut out, last);
            offset += read;
            if result == CoderResult::InputEmpty {
                return Some(out);
            }
        }
    }
}

/// `op_encoding_encode(text) -> ArrayBuffer` (UTF-8; lone surrogates become U+FFFD).
fn op_encoding_encode(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError(
            "op_encoding_encode expects (text: string)".into(),
        ));
    };
    Ok(ctx.create_array_buffer(text.to_string_lossy().as_bytes()))
}

/// `op_encoding_encode_into(text, capacity) -> [read, ArrayBuffer]`
///
/// Encodes as many whole characters as fit in `capacity` bytes. `read` counts UTF-16 units.
fn op_encoding_encode_into(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text), Value::Number(capacity)] = args else {
        return Err(HostError::TypeError(
            "op_encoding_encode_into expects (text: string, capacity: number)".into(),
        ));
    };
    let capacity = *capacity as usize;
    let mut bytes = Vec::new();
    let mut read = 0;
    for unit in char::decode_utf16(text.as_utf16().iter().copied()) {
        let ch = unit.unwrap_or(char::REPLACEMENT_CHARACTER);
        if bytes.len() + ch.len_utf8() > capacity {
            break;
        }
        bytes.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
        read += ch.len_utf16();
    }
    let buffer = ctx.create_array_buffer(&bytes);
    Ok(ctx.create_array(&[Value::Number(read as f64), buffer]))
}

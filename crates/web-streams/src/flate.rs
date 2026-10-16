use std::collections::HashMap;
use std::io::{self, Write};

use flate2::write::{DeflateEncoder, GzDecoder, GzEncoder, ZlibEncoder};
use flate2::{Compression, Decompress, FlushDecompress, Status};
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};

/// One live stage per stream id; the JS `CompressionStream` / `DecompressionStream` hold the id.
#[derive(Default)]
pub(crate) struct FlateTable(HashMap<u32, Box<dyn Stage>>);

/// One direction of one stream: feed it chunks, then finish it. Both calls return the bytes produced.
trait Stage {
    fn feed(&mut self, data: &[u8]) -> io::Result<Vec<u8>>;
    fn end(self: Box<Self>) -> io::Result<Vec<u8>>;
}

/// The flate2 write adaptors buffer into a `Vec<u8>` that is drained after each write.
macro_rules! write_stage {
    ($($ty:ty),* $(,)?) => {$(
        impl Stage for $ty {
            fn feed(&mut self, data: &[u8]) -> io::Result<Vec<u8>> {
                self.write_all(data)?;
                Ok(std::mem::take(self.get_mut()))
            }

            fn end(self: Box<Self>) -> io::Result<Vec<u8>> {
                self.finish()
            }
        }
    )*};
}

write_stage!(
    GzEncoder<Vec<u8>>,
    ZlibEncoder<Vec<u8>>,
    DeflateEncoder<Vec<u8>>,
    GzDecoder<Vec<u8>>,
);

/// zlib / raw deflate decoding with the low-level API: unlike `write::ZlibDecoder` it can tell that
/// the stream ended, so truncated input and trailing bytes are errors (the spec requires a `TypeError`).
struct Inflate {
    inner: Decompress,
    ended: bool,
}

const MIN_ROOM: usize = 4096;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl Inflate {
    fn new(zlib_header: bool) -> Self {
        Self {
            inner: Decompress::new(zlib_header),
            ended: false,
        }
    }

    /// Runs the decoder until it needs more input or has no more output.
    fn run(&mut self, mut input: &[u8], flush: FlushDecompress) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            out.reserve(MIN_ROOM.max(input.len().saturating_mul(2)));
            let before = self.inner.total_in();
            let status = self
                .inner
                .decompress_vec(input, &mut out, flush)
                .map_err(|error| invalid(&error.to_string()))?;
            let consumed = (self.inner.total_in() - before) as usize;
            input = &input[consumed..];
            if status == Status::StreamEnd {
                self.ended = true;
                return if input.is_empty() {
                    Ok(out)
                } else {
                    Err(invalid("trailing data after the end of the stream"))
                };
            }
            let output_full = out.len() == out.capacity();
            if !output_full {
                return if input.is_empty() {
                    Ok(out)
                } else {
                    Err(invalid("corrupt compressed data"))
                };
            }
        }
    }
}

impl Stage for Inflate {
    fn feed(&mut self, data: &[u8]) -> io::Result<Vec<u8>> {
        if self.ended && !data.is_empty() {
            return Err(invalid("trailing data after the end of the stream"));
        }
        self.run(data, FlushDecompress::None)
    }

    fn end(mut self: Box<Self>) -> io::Result<Vec<u8>> {
        let out = self.run(&[], FlushDecompress::Finish)?;
        if self.ended {
            Ok(out)
        } else {
            Err(invalid("unexpected end of the compressed data"))
        }
    }
}

fn new_stage(compress: bool, format: &str) -> Option<Box<dyn Stage>> {
    let level = Compression::default();
    Some(match (compress, format) {
        (true, "gzip") => Box::new(GzEncoder::new(Vec::new(), level)),
        (true, "deflate") => Box::new(ZlibEncoder::new(Vec::new(), level)),
        (true, "deflate-raw") => Box::new(DeflateEncoder::new(Vec::new(), level)),
        (false, "gzip") => Box::new(GzDecoder::new(Vec::new())),
        (false, "deflate") => Box::new(Inflate::new(true)),
        (false, "deflate-raw") => Box::new(Inflate::new(false)),
        _ => return None,
    })
}

pub fn flate_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_streams_flate",
            arity: 5,
            callback: op_streams_flate,
        },
        HostFunctionDef {
            name: "op_streams_flate_free",
            arity: 1,
            callback: op_streams_flate_free,
        },
    ]
}

/// `op_streams_flate(compress, format, id, bytes, finish) -> ArrayBuffer`
///
/// `bytes` is a binary string. The codec for `id` is created on first use and dropped on `finish`
/// or on any error, so a failed stream leaves nothing behind.
fn op_streams_flate(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [
        Value::Bool(compress),
        Value::String(format),
        Value::Number(id),
        Value::String(bytes),
        Value::Bool(finish),
    ] = args
    else {
        return Err(HostError::TypeError(
            "op_streams_flate expects (compress, format, id, bytes, finish)".into(),
        ));
    };
    let bytes: Vec<u8> = bytes.as_utf16().iter().map(|&unit| unit as u8).collect();
    let id = *id as u32;
    let table = ctx
        .isolate_slot::<FlateTable>()
        .ok_or_else(|| HostError::TypeError("streams bootstrap is not installed".into()))?;
    let mut stage = match table.0.remove(&id) {
        Some(stage) => stage,
        None => new_stage(*compress, &format.to_string_lossy())
            .ok_or_else(|| HostError::TypeError("unsupported compression format".into()))?,
    };
    let result = stage.feed(&bytes).and_then(|mut out| {
        if *finish {
            out.extend(stage.end()?);
            return Ok((out, None));
        }
        Ok((out, Some(stage)))
    });
    match result {
        Ok((out, keep)) => {
            if let Some(stage) = keep {
                table.0.insert(id, stage);
            }
            Ok(ctx.create_array_buffer(&out))
        }
        Err(error) => {
            Err(ctx.throw_type_error(&format!("The compressed data is not valid: {error}")))
        }
    }
}

/// `op_streams_flate_free(id)`: drops the codec of a cancelled or aborted stream.
fn op_streams_flate_free(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::Number(id)] = args else {
        return Err(HostError::TypeError(
            "op_streams_flate_free expects (id: number)".into(),
        ));
    };
    if let Some(table) = ctx.isolate_slot::<FlateTable>() {
        table.0.remove(&(*id as u32));
    }
    Ok(Value::Undefined)
}

//! Host ops behind `node:zlib`. Compression state lives in the isolate's resource table (JS holds
//! only an id), so streams do incremental work: each `zlib_write` feeds one chunk and returns
//! whatever output is ready. The codecs and the zip-bomb output limit are in `zlib_codec`.
//!
//! Ops (name plus typed arrays, result framed like `crypto_ops`). With `shared` = `level, quality,
//! limit, window_bits, dictionary`: `zlib_new [kind, shared]`, `zlib_write [id, data, mode]` (`0`
//! data, `1` sync flush, `2` finish and close), `zlib_params [id, level]`, `zlib_reset [id]`,
//! `zlib_free [id]`, `zlib_once [kind, shared, data]` (stateless, also on the blocking pool) and
//! `crc32 [data, initial]`.

use std::any::Any;
use std::cell::RefCell;
use std::io;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{OpContext, op_async};
use velyr_runtime_resources::{QuotaClass, Resource, ResourceId, ResourceTable};

use crate::crypto_ops::{Parts, encode, read_call};
use crate::zlib_codec::{Params, Stage, failure, once, stage};

const CONSUMED: &str = "ERR_CRYPTO_INVALID_STATE|zlib stream already finished";

pub(crate) fn zlib_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_node_zlib",
            arity: 1,
            callback: op_sync,
        },
        HostFunctionDef {
            name: "op_node_zlib_async",
            arity: 1,
            callback: op_async_call,
        },
    ]
}

fn number(bytes: &[u8]) -> Result<u64, String> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .ok_or_else(|| "Invalid numeric argument".to_string())
}

/// The shared codec arguments, in wire order. An empty `dictionary` means none.
fn params(args: &[Vec<u8>]) -> Result<Params, String> {
    let [level, quality, limit, bits, dictionary] = args else {
        return Err("zlib: wrong arguments".into());
    };
    let level = std::str::from_utf8(level)
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .ok_or("Invalid numeric argument")?;
    Ok(Params {
        level,
        quality: number(quality)?.min(11) as u32,
        limit: usize::try_from(number(limit)?).map_err(|_| "Invalid limit".to_string())?,
        window_bits: number(bits)?.min(15) as u8,
        dictionary: (!dictionary.is_empty()).then(|| dictionary.clone()),
    })
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn crc32(data: &[u8], initial: &[u8]) -> Parts {
    let initial = u32::try_from(number(initial)?).map_err(|_| "Invalid crc32 seed".to_string())?;
    let mut hasher = crc32fast::Hasher::new_with_initial(initial);
    hasher.update(data);
    Ok(vec![hasher.finalize().to_string().into_bytes()])
}

fn dispatch_stateless(name: &str, args: &[Vec<u8>]) -> Option<Parts> {
    match (name, args) {
        ("zlib_once", [kind, shared @ .., data]) => Some(
            params(shared).and_then(|params| once(&text(kind), &params, data).map(|out| vec![out])),
        ),
        ("crc32", [data, initial]) => Some(crc32(data, initial)),
        _ => None,
    }
}

struct ZlibContext {
    stage: RefCell<Option<Box<dyn Stage>>>,
    /// Kept so `zlib_reset` can build a fresh codec of the same kind.
    kind: String,
    /// `zlib_params` updates the level, so a later reset keeps it.
    params: RefCell<Params>,
    limit: usize,
}

impl Resource for ZlibContext {
    fn kind(&self) -> &'static str {
        "zlib.context"
    }
    fn close(self: Box<Self>) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn parse_id(bytes: &[u8]) -> Result<ResourceId, String> {
    let value: f64 = text(bytes)
        .parse()
        .map_err(|_| "Invalid context id".to_string())?;
    ResourceId::from_js_number(value).map_err(|error| error.to_string())
}

fn write(table: &mut ResourceTable, id: &[u8], data: &[u8], mode: &[u8]) -> Parts {
    let zlib = table
        .get::<ZlibContext>(parse_id(id)?)
        .map_err(|error| error.to_string())?;
    let limit = zlib.limit;
    let fail = move |error: io::Error| failure(&error, limit);
    if mode == b"2" {
        let mut stage = zlib.stage.borrow_mut().take().ok_or(CONSUMED)?;
        let result = stage.feed(data).and_then(|()| stage.finish());
        let _ = table.close(parse_id(id)?);
        return Ok(vec![result.map_err(fail)?.buf]);
    }
    let mut slot = zlib.stage.borrow_mut();
    let stage = slot.as_mut().ok_or(CONSUMED)?;
    stage.feed(data).map_err(fail)?;
    if mode == b"1" {
        stage.sync().map_err(fail)?;
    }
    Ok(vec![std::mem::take(&mut stage.sink().buf)])
}

fn stateful(ctx: &mut dyn CallContext, name: &str, args: &[Vec<u8>]) -> Parts {
    let table = &mut ctx
        .isolate_slot::<OpContext>()
        .ok_or("zlib contexts need an installed OpContext")?
        .resources;
    match (name, args) {
        ("zlib_new", [kind, shared @ ..]) => {
            let params = params(shared)?;
            let kind = text(kind);
            let resource = Box::new(ZlibContext {
                stage: RefCell::new(Some(stage(&kind, &params)?)),
                limit: params.limit,
                kind,
                params: RefCell::new(params),
            });
            let id = table
                .insert(resource, QuotaClass::Compression, None)
                .map_err(|error| error.to_string())?;
            Ok(vec![id.to_js_number().to_string().into_bytes()])
        }
        // Drops the codec state (and pending output) for a fresh one of the same kind and options.
        ("zlib_reset", [id]) => {
            let zlib = table
                .get::<ZlibContext>(parse_id(id)?)
                .map_err(|error| error.to_string())?;
            *zlib.stage.borrow_mut() = Some(stage(&zlib.kind, &zlib.params.borrow())?);
            Ok(Vec::new())
        }
        // `params()`: flush, switch the level, return what the flush produced.
        ("zlib_params", [id, level]) => {
            let zlib = table
                .get::<ZlibContext>(parse_id(id)?)
                .map_err(|error| error.to_string())?;
            let level: i32 = text(level)
                .parse()
                .map_err(|_| "Invalid level".to_string())?;
            let mut slot = zlib.stage.borrow_mut();
            let stage = slot.as_mut().ok_or(CONSUMED)?;
            stage
                .set_level(level)
                .map_err(|error| failure(&error, zlib.limit))?;
            zlib.params.borrow_mut().level = level;
            Ok(vec![std::mem::take(&mut stage.sink().buf)])
        }
        ("zlib_write", [id, data, mode]) => write(table, id, data, mode),
        // Frees abandoned streams (JS finalizers); a finished one is not an error.
        ("zlib_free", [id]) => {
            let _ = table.close(parse_id(id)?);
            Ok(Vec::new())
        }
        _ => Err(format!("{name}: wrong arguments")),
    }
}

fn op_sync(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (name, parts) = read_call(ctx, args, "zlib")?;
    let result = dispatch_stateless(&name, &parts).unwrap_or_else(|| stateful(ctx, &name, &parts));
    Ok(ctx.create_array_buffer(&encode(result)))
}

fn op_async_call(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (name, parts) = read_call(ctx, args, "zlib")?;
    op_async(ctx, |_token| async move {
        let result = tokio::task::spawn_blocking(move || {
            dispatch_stateless(&name, &parts).unwrap_or_else(|| Err(format!("{name}: not async")))
        })
        .await
        .map_err(|err| VelyrError::internal(err.to_string()))?;
        Ok(CompletionValue::Bytes(encode(result)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(kind: &str, data: &[u8], limit: &str) -> Parts {
        dispatch_stateless(
            "zlib_once",
            &[
                kind.into(),
                b"-1".to_vec(),
                b"4".to_vec(),
                limit.into(),
                b"15".to_vec(),
                Vec::new(),
                data.to_vec(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn every_format_round_trips() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 97) as u8).collect();
        for (pack, unpack) in [
            ("gzip", "gunzip"),
            ("deflate", "inflate"),
            ("deflateRaw", "inflateRaw"),
            ("brotliCompress", "brotliDecompress"),
        ] {
            let packed = run(pack, &data, "1048576").unwrap().remove(0);
            assert!(packed.len() < data.len(), "{pack} shrinks the input");
            assert_eq!(
                run(unpack, &packed, "1048576").unwrap().remove(0),
                data,
                "{pack}"
            );
        }
    }

    #[test]
    fn output_limit_stops_a_bomb_and_damaged_input_is_reported() {
        let bomb = run("gzip", &vec![0u8; 4 << 20], "8388608")
            .unwrap()
            .remove(0);
        assert!(bomb.len() < 10_000, "the bomb is small");
        let error = run("gunzip", &bomb, "65536").unwrap_err();
        assert!(error.starts_with("ERR_BUFFER_TOO_LARGE|"), "{error}");
        for (pack, unpack) in [
            ("deflate", "inflate"),
            ("gzip", "gunzip"),
            ("brotliCompress", "brotliDecompress"),
        ] {
            let packed = run(pack, b"hello hello hello", "1048576")
                .unwrap()
                .remove(0);
            let error = run(unpack, &packed[..packed.len() - 3], "1048576").unwrap_err();
            assert!(error.starts_with("Z_BUF_ERROR|"), "{unpack}: {error}");
        }
        let error = run("gunzip", b"definitely not gzip data", "1048576").unwrap_err();
        assert!(error.starts_with("Z_DATA_ERROR|"), "{error}");
        assert!(
            run("gunzip", b"", "1048576")
                .unwrap_err()
                .starts_with("Z_BUF_ERROR|")
        );
    }

    #[test]
    fn crc32_matches_the_check_value_and_chains() {
        let crc = |data: &[u8], seed: &[u8]| text(&crc32(data, seed).unwrap().remove(0));
        assert_eq!(crc(b"123456789", b"0"), 0xcbf4_3926u32.to_string());
        let first = crc(b"1234", b"0");
        assert_eq!(crc(b"56789", first.as_bytes()), 0xcbf4_3926u32.to_string());
    }
}

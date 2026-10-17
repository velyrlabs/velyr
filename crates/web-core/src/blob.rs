//! Backing store for `Blob`: the bytes stay in Rust, JS holds a numeric id. A slice is a second
//! id over the same allocation, so slicing never copies.

use std::collections::HashMap;
use std::sync::Arc;

use velyr_engine_api::{
    CallContext, HostError, HostFunctionDef, Isolate, JsException, Source, Value,
};

/// Bootstrap script: `Blob`, `File` and `URL.createObjectURL` on top of the private `op_blob_*`
/// binding. Runs after [`crate::URL_JS`].
pub const BLOB_JS: &str = include_str!("blob.js");

#[derive(Clone)]
struct Piece {
    data: Arc<[u8]>,
    start: usize,
    end: usize,
}

#[derive(Default)]
struct BlobStore {
    next: u64,
    pieces: HashMap<u64, Piece>,
}

impl BlobStore {
    fn insert(&mut self, piece: Piece) -> f64 {
        self.next += 1;
        self.pieces.insert(self.next, piece);
        self.next as f64
    }
}

pub fn blob_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_blob_create",
            arity: 1,
            callback: op_blob_create,
        },
        HostFunctionDef {
            name: "op_blob_slice",
            arity: 3,
            callback: op_blob_slice,
        },
        HostFunctionDef {
            name: "op_blob_read",
            arity: 3,
            callback: op_blob_read,
        },
        HostFunctionDef {
            name: "op_blob_free",
            arity: 1,
            callback: op_blob_free,
        },
    ]
}

/// Registers the store and runs [`BLOB_JS`]. Call after `install_url`, with the
/// `blob_host_functions` registered.
pub fn install_blob(isolate: &mut impl Isolate) -> Result<(), JsException> {
    install_blob_slots(isolate);
    isolate
        .eval_script(&Source {
            name: "velyr:blob".into(),
            text: BLOB_JS.into(),
        })
        .map(drop)
}

/// The Rust half of [`install_blob`], for an isolate whose [`BLOB_JS`] already ran.
pub fn install_blob_slots(isolate: &mut impl Isolate) {
    isolate.set_slot(BlobStore::default());
}

fn store(ctx: &mut dyn CallContext) -> Result<&mut BlobStore, HostError> {
    ctx.isolate_slot::<BlobStore>()
        .ok_or_else(|| HostError::TypeError("Blob needs an installed blob store".into()))
}

fn index_arg(args: &[Value], index: usize, op: &str) -> Result<usize, HostError> {
    match args.get(index) {
        Some(Value::Number(n)) if *n >= 0.0 && n.fract() == 0.0 && *n <= u32::MAX as f64 => {
            Ok(*n as usize)
        }
        _ => Err(HostError::TypeError(format!(
            "{op} expects argument {index} to be a non-negative integer"
        ))),
    }
}

fn piece_of(ctx: &mut dyn CallContext, args: &[Value], op: &str) -> Result<Piece, HostError> {
    let id = index_arg(args, 0, op)? as u64;
    store(ctx)?
        .pieces
        .get(&id)
        .cloned()
        .ok_or_else(|| HostError::TypeError(format!("{op}: unknown blob")))
}

/// `op_blob_create(bytes) -> id`: copies an ArrayBuffer or view into the store.
fn op_blob_create(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let bytes = args
        .first()
        .and_then(|value| ctx.read_bytes(value))
        .ok_or_else(|| {
            HostError::TypeError("op_blob_create expects an ArrayBuffer or view".into())
        })?;
    let end = bytes.len();
    let data: Arc<[u8]> = bytes.into();
    let id = store(ctx)?.insert(Piece {
        data,
        start: 0,
        end,
    });
    Ok(Value::Number(id))
}

/// `op_blob_slice(id, start, end) -> id`: a view of bytes `start..end` (clamped) of a blob.
fn op_blob_slice(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let piece = piece_of(ctx, args, "op_blob_slice")?;
    let len = piece.end - piece.start;
    let start = index_arg(args, 1, "op_blob_slice")?.min(len);
    let end = index_arg(args, 2, "op_blob_slice")?.clamp(start, len);
    let id = store(ctx)?.insert(Piece {
        data: piece.data,
        start: piece.start + start,
        end: piece.start + end,
    });
    Ok(Value::Number(id))
}

/// `op_blob_read(id, offset, length) -> ArrayBuffer`: a copy of at most `length` bytes.
fn op_blob_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let piece = piece_of(ctx, args, "op_blob_read")?;
    let len = piece.end - piece.start;
    let offset = index_arg(args, 1, "op_blob_read")?.min(len);
    let length = index_arg(args, 2, "op_blob_read")?.min(len - offset);
    let from = piece.start + offset;
    Ok(ctx.create_array_buffer(&piece.data[from..from + length]))
}

/// `op_blob_free(id)`: forgets a blob (its bytes go once no slice shares them).
fn op_blob_free(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = index_arg(args, 0, "op_blob_free")? as u64;
    store(ctx)?.pieces.remove(&id);
    Ok(Value::Undefined)
}

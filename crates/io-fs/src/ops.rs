use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{CapabilityKind, authorize, blocking, op_async};
use velyr_runtime_permissions::Request;

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 3] = [
    (
        HostFunctionDef {
            name: "op_fs_read",
            arity: 2,
            callback: op_fs_read,
        },
        CapabilityKind::FsRead,
    ),
    (
        HostFunctionDef {
            name: "op_fs_write",
            arity: 3,
            callback: op_fs_write,
        },
        CapabilityKind::FsWrite,
    ),
    (
        HostFunctionDef {
            name: "op_fs_stat",
            arity: 1,
            callback: op_fs_stat,
        },
        CapabilityKind::FsRead,
    ),
];

fn io_error(err: io::Error) -> VelyrError {
    match err.kind() {
        io::ErrorKind::NotFound => VelyrError::not_found("no such file or directory"),
        io::ErrorKind::AlreadyExists => VelyrError::already_exists("file already exists"),
        io::ErrorKind::PermissionDenied => VelyrError::new(
            ErrorCode::PermissionDenied,
            "the operating system denied access",
        ),
        io::ErrorKind::InvalidInput => VelyrError::invalid_data("invalid path"),
        io::ErrorKind::IsADirectory => VelyrError::invalid_data("path is a directory"),
        io::ErrorKind::NotADirectory => VelyrError::invalid_data("path is not a directory"),
        _ => VelyrError::internal(err.to_string()),
    }
}

/// Canonical path of `raw`; for a write whose target does not exist yet, the canonical parent
/// plus the file name. A dangling symlink is refused: writing through it would create a file
/// at a location that was never authorized.
fn canonicalize(raw: &str, for_write: bool) -> Result<PathBuf, VelyrError> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(VelyrError::invalid_data("invalid path"));
    }
    let path = Path::new(raw);
    match dunce::canonicalize(path) {
        Ok(canonical) => Ok(canonical),
        Err(err) if for_write && err.kind() == io::ErrorKind::NotFound => {
            if path.symlink_metadata().is_ok() {
                return Err(VelyrError::new(
                    ErrorCode::PermissionDenied,
                    "write through a dangling symlink is not allowed",
                ));
            }
            let name = path
                .file_name()
                .ok_or_else(|| VelyrError::invalid_data("invalid path"))?;
            let parent = match path.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent,
                _ => Path::new("."),
            };
            Ok(dunce::canonicalize(parent).map_err(io_error)?.join(name))
        }
        Err(err) => Err(io_error(err)),
    }
}

/// Canonicalizes `raw`, authorizes the canonical path and returns it. TOCTOU: the decision holds
/// for the filesystem as it is now, so callers must open exactly the returned path and never
/// re-resolve `raw`. A path that cannot be resolved is authorized as given first, so a caller
/// without the grant always sees a denial and never learns whether the file exists.
fn authorized_path(
    ctx: &mut dyn CallContext,
    op: &'static str,
    args: &[Value],
    for_write: bool,
) -> Result<PathBuf, HostError> {
    let Some(Value::String(raw)) = args.first() else {
        return Err(HostError::TypeError(format!("{op} expects a path string")));
    };
    authorized_raw(ctx, op, &raw.to_string_lossy(), for_write)
}

/// [`authorized_path`] for a path that is already a Rust string.
pub(crate) fn authorized_raw(
    ctx: &mut dyn CallContext,
    op: &'static str,
    raw: &str,
    for_write: bool,
) -> Result<PathBuf, HostError> {
    let request = |path| {
        if for_write {
            Request::FsWrite(path)
        } else {
            Request::FsRead(path)
        }
    };
    let canonical = match canonicalize(raw, for_write) {
        Ok(canonical) => canonical,
        Err(err) => {
            authorize(ctx, op, request(raw))?;
            return Err(err.into());
        }
    };
    let text = canonical
        .to_str()
        .ok_or_else(|| VelyrError::invalid_data("path is not valid UTF-8"))?;
    authorize(ctx, op, request(text))?;
    Ok(canonical)
}

fn flag(args: &[Value], index: usize, op: &str) -> Result<bool, HostError> {
    match args.get(index) {
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(HostError::TypeError(format!(
            "{op} expects argument {index} to be a boolean"
        ))),
    }
}

/// `op_fs_read(path, text) -> Promise<string | ArrayBuffer>`: lossy UTF-8 text, or the raw bytes
/// when `text` is false.
fn op_fs_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.fs_read").entered();
    let text = flag(args, 1, "op_fs_read")?;
    if let Some(Value::String(raw)) = args.first() {
        let raw = raw.to_string_lossy();
        // A compiled executable's embedded files: same FsRead check, read from the payload.
        if let Some(vfs) = velyr_runtime_vfs::covering(Path::new(&raw)) {
            authorize(ctx, "op_fs_read", Request::FsRead(&raw))?;
            let embedded = vfs.read(&raw).map(<[u8]>::to_vec);
            return op_async(ctx, |_token| async move {
                let bytes = embedded.ok_or_else(|| io_error(io::ErrorKind::NotFound.into()))?;
                Ok(read_value(bytes, text))
            });
        }
    }
    let path = authorized_path(ctx, "op_fs_read", args, false)?;
    op_async(ctx, |_token| async move {
        let bytes = blocking(move || std::fs::read(path))
            .await?
            .map_err(io_error)?;
        Ok(read_value(bytes, text))
    })
}

fn read_value(bytes: Vec<u8>, text: bool) -> CompletionValue {
    if text {
        CompletionValue::String(String::from_utf8_lossy(&bytes).into_owned())
    } else {
        CompletionValue::Bytes(bytes)
    }
}

/// `op_fs_write(path, data, text) -> Promise<undefined>`; `data` is UTF-16 text, or an
/// `ArrayBuffer`/`ArrayBufferView` when `text` is false. Creates or truncates the file.
fn op_fs_write(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.fs_write").entered();
    let path = authorized_path(ctx, "op_fs_write", args, true)?;
    let text = flag(args, 2, "op_fs_write")?;
    let bytes = match (text, args.get(1)) {
        (true, Some(Value::String(data))) => data.to_string_lossy().into_bytes(),
        (false, Some(data)) => ctx.read_bytes(data).ok_or_else(|| {
            HostError::TypeError(
                "op_fs_write expects argument 1 to be an ArrayBuffer or ArrayBufferView".into(),
            )
        })?,
        _ => {
            return Err(HostError::TypeError(
                "op_fs_write expects argument 1 to be a string".into(),
            ));
        }
    };
    op_async(ctx, |_token| async move {
        blocking(move || std::fs::write(path, bytes))
            .await?
            .map_err(io_error)?;
        Ok(CompletionValue::Undefined)
    })
}

/// `op_fs_stat(path) -> Promise<string>`: JSON `{size, isFile, isDirectory, mtimeMs}`.
fn op_fs_stat(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.fs_stat").entered();
    let path = authorized_path(ctx, "op_fs_stat", args, false)?;
    op_async(ctx, |_token| async move {
        let meta = blocking(move || std::fs::metadata(path))
            .await?
            .map_err(io_error)?;
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_secs_f64() * 1000.0);
        let json = serde_json::json!({
            "size": meta.len(),
            "isFile": meta.is_file(),
            "isDirectory": meta.is_dir(),
            "mtimeMs": mtime_ms,
        });
        Ok(CompletionValue::String(json.to_string()))
    })
}

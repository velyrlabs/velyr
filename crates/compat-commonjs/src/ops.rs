//! The `op_cjs_*` host functions behind `require`. Both are synchronous (`require` is), so they
//! read with `std::fs` after the same `ImportLocal`/`FsRead` check `ModuleHost::load` makes.

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use velyr_engine_api::{
    CallContext, HostError, HostFunctionDef, JsString, Source, Value, function_digest,
};
use velyr_runtime_core::require_esm;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{MediaType, ModuleId, SharedCache, SourceTransform, authorize_path};
use velyr_runtime_ops::CapabilityKind;
use velyr_runtime_permissions::CapabilitySet;

use crate::analysis::{analyze, rewrite_dynamic_imports};
use crate::format::Format;
use crate::fscache;
use crate::resolve::resolve;

/// Isolate slot the ops read.
pub(crate) struct CommonJsState {
    pub permissions: Arc<CapabilitySet>,
    pub conditions: Vec<String>,
    /// Strips TypeScript from the files `op_cjs_read` returns; `None` leaves them as read.
    pub strip: Option<Arc<dyn SourceTransform>>,
}

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 4] = [
    (
        HostFunctionDef {
            name: "op_cjs_resolve",
            arity: 2,
            callback: op_cjs_resolve,
        },
        CapabilityKind::ImportLocal,
    ),
    (
        HostFunctionDef {
            name: "op_cjs_read",
            arity: 1,
            callback: op_cjs_read,
        },
        CapabilityKind::ImportLocal,
    ),
    (
        HostFunctionDef {
            name: "op_cjs_require_esm",
            arity: 1,
            callback: op_cjs_require_esm,
        },
        CapabilityKind::ImportLocal,
    ),
    (
        HostFunctionDef {
            name: "op_cjs_compile",
            arity: 2,
            callback: op_cjs_compile,
        },
        CapabilityKind::ImportLocal,
    ),
];

/// Parameters of a compiled CommonJS module; the sixth is the rewritten dynamic `import()`.
const WRAPPER_PARAMS: [&str; 6] = [
    "exports",
    "require",
    "module",
    "__filename",
    "__dirname",
    "__velyrImport",
];

fn string_arg(args: &[Value], index: usize, op: &str) -> Result<String, HostError> {
    match args.get(index) {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        _ => Err(HostError::TypeError(format!(
            "{op} expects argument {index} to be a string"
        ))),
    }
}

fn state(ctx: &mut dyn CallContext) -> Result<(Arc<CapabilitySet>, Vec<String>), HostError> {
    ctx.isolate_slot::<CommonJsState>()
        .map(|s| (s.permissions.clone(), s.conditions.clone()))
        .ok_or_else(|| HostError::TypeError("CommonJS ops need install_commonjs".into()))
}

fn read_error(err: std::io::Error) -> VelyrError {
    match err.kind() {
        std::io::ErrorKind::NotFound => VelyrError::not_found("module not found"),
        _ => VelyrError::invalid_data("module is not readable UTF-8 text").with_cause(err),
    }
}

fn text(value: &str) -> Value {
    Value::String(JsString::from(value))
}

/// `op_cjs_resolve(request, parentFile) -> '{"path": ..., "format": "commonjs"|"json"|"esm"}'`.
fn op_cjs_resolve(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let request = string_arg(args, 0, "op_cjs_resolve")?;
    let parent = string_arg(args, 1, "op_cjs_resolve")?;
    let (permissions, conditions) = state(ctx)?;
    let auth = |p: &Path| authorize_path(p, &permissions);
    let path = resolve(&request, Path::new(&parent), &conditions, &auth)?;
    authorize_path(&path, &permissions)?;
    let format = fscache::global().classify(&path, &auth);
    let path = path.to_string_lossy();
    Ok(text(
        &serde_json::json!({ "path": path, "format": format.as_str() }).to_string(),
    ))
}

/// `op_cjs_read(path) -> source`; TypeScript is stripped first (the only strip site, so the text
/// V8 compiles and the code-cache key are the stripped text), then CommonJS text has its
/// `import(` calls routed through the wrapper's `__velyrImport`.
fn op_cjs_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let path = string_arg(args, 0, "op_cjs_read")?;
    let (permissions, strip) = {
        let state = ctx
            .isolate_slot::<CommonJsState>()
            .ok_or_else(|| HostError::TypeError("CommonJS ops need install_commonjs".into()))?;
        (state.permissions.clone(), state.strip.clone())
    };
    let path = Path::new(&path);
    authorize_path(path, &permissions)?;
    let read = fscache::global()
        .read(path, &|p| authorize_path(p, &permissions))
        .map_err(read_error)?;
    if read.format == Format::Json {
        return Ok(text(&read.text));
    }
    let source = strip_types(strip.as_deref(), path, &read.text)?;
    if !source.contains("import") {
        return Ok(text(&source));
    }
    let offsets = analyze(&source)
        .map(|a| a.dynamic_imports)
        .unwrap_or_default();
    Ok(text(&rewrite_dynamic_imports(&source, &offsets)))
}

/// `.jsx` shares `MediaType::JavaScript` with plain `.js`, so the extension tells it apart.
fn needs_strip(path: &Path, media_type: MediaType) -> bool {
    media_type != MediaType::JavaScript || path.extension().is_some_and(|ext| ext == "jsx")
}

/// `raw` with its types removed when `path` is TypeScript and a `strip` step is installed.
fn strip_types<'a>(
    strip: Option<&dyn SourceTransform>,
    path: &Path,
    raw: &'a str,
) -> Result<Cow<'a, str>, HostError> {
    let media_type = MediaType::from_path(path);
    match strip {
        Some(strip) if needs_strip(path, media_type) && strip.handles(media_type) => {
            let code = strip
                .transform(&path.to_string_lossy(), raw, media_type)?
                .code;
            Ok(Cow::Owned(code))
        }
        _ => Ok(Cow::Borrowed(raw)),
    }
}

/// `op_cjs_require_esm(path) -> namespace`, or `undefined` when the module has top-level await
/// (a namespace is never `undefined`). The graph loads through the runtime's `EsmRequire`.
fn op_cjs_require_esm(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let path = string_arg(args, 0, "op_cjs_require_esm")?;
    Ok(require_esm(ctx, Path::new(&path))?.unwrap_or(Value::Undefined))
}

/// `op_cjs_compile(path, text) -> function`: `text` compiled as the body of a function of
/// [`WRAPPER_PARAMS`], through the shared code cache when it is on. Engines without
/// `compile_function` answer `VELYR_UNSUPPORTED` and the loader falls back to `new Function`.
fn op_cjs_compile(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let path = string_arg(args, 0, "op_cjs_compile")?;
    let source = Source {
        name: path,
        text: string_arg(args, 1, "op_cjs_compile")?,
    };
    let shared = SharedCache::global()
        .zip(ModuleId::from_path(Path::new(&source.name)))
        .map(|(shared, id)| {
            let key = format!("fn:{:016x}", function_digest(&WRAPPER_PARAMS, &source.text));
            (shared, id, key)
        });
    let cached = shared
        .as_ref()
        .and_then(|(shared, id, key)| shared.code(id, key));
    let function = ctx.compile_function(&source, &WRAPPER_PARAMS, cached.as_deref())?;
    if let (None, Some((shared, id, key))) = (cached, shared) {
        shared.note_function(&id, &key);
    }
    Ok(function)
}

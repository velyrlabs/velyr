//! Startup snapshots (VELYR-123, VELYR-232): a context with host functions installed and bootstrap
//! scripts already evaluated, so an isolate booted from it (see [`crate::V8Engine::with_snapshot`])
//! skips that work.

use std::borrow::Cow;

use v8::MapFnTo;
use velyr_engine_api::{EngineError, HostFunctionDef, Source};
use velyr_runtime_errors::VelyrError;

use crate::shared::Shared;
use crate::{V8Engine, host, modules, script_origin};

/// `Symbol.for(SNAPSHOT_RESULTS_KEY)` on the snapshot's global holds a null-prototype object with
/// the completion value of each snapshot script, by script name (`undefined` ones left out). The
/// embedder reads what it needs and deletes the property before user code runs.
pub const SNAPSHOT_RESULTS_KEY: &str = "velyr.snapshot";

/// Context data slot holding the names of the host functions, in registration order.
const OP_NAMES: usize = 0;

/// Creates an isolate whose contexts start from `blob` (see [`create_snapshot`]).
pub(crate) fn boot(params: v8::CreateParams, blob: &'static [u8]) -> v8::OwnedIsolate {
    let params = params
        .snapshot_blob(blob.into())
        .external_references(external_references());
    v8::Isolate::new(params)
}

/// Every native callback V8 can meet in a serialized heap. Creation and boot must pass the same
/// table: V8 aborts the process on a callback missing from it.
fn external_references() -> Cow<'static, [v8::ExternalReference]> {
    Cow::Owned(vec![
        v8::ExternalReference {
            function: host::trampoline.map_fn_to(),
        },
        v8::ExternalReference {
            function: modules::return_data.map_fn_to(),
        },
    ])
}

/// Installs `host_functions` on the global object, runs `scripts` in order and serializes the
/// context as the snapshot's default context. The scripts may capture the host functions but
/// not call them: no isolate state exists yet, so a call throws. Boot the blob with the same
/// `host_functions`, in the same order ([`crate::V8Engine::with_snapshot`]); a script's frames
/// keep its `Source::name`.
///
/// Meant for build scripts: run it alone in its process. V8 crashes or corrupts heaps when it
/// runs concurrently with another snapshot creation or with isolates booted from a snapshot.
pub fn create_snapshot(
    host_functions: &[HostFunctionDef],
    scripts: &[Source],
) -> Result<Vec<u8>, EngineError> {
    V8Engine::init_platform();
    // Declared before `isolate` so it drops after it: forgets the tasks V8 posted for it.
    let _platform_queue;
    let mut isolate = v8::Isolate::snapshot_creator(Some(external_references()), None);
    _platform_queue = crate::platform::QueueGuard(crate::platform::key(&isolate));
    let ran = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, Default::default());
        let scope = &mut v8::ContextScope::new(scope, context);
        scope.set_default_context(context);
        populate(scope, context, host_functions, scripts)
    };
    // A snapshot creator must reach `create_blob` even on failure: dropping it panics.
    let blob = isolate.create_blob(v8::FunctionCodeHandling::Keep);
    ran?;
    blob.map(|blob| blob.to_vec())
        .ok_or_else(|| VelyrError::internal("V8 could not create the snapshot blob").into())
}

fn populate(
    scope: &mut v8::PinScope<'_, '_>,
    context: v8::Local<'_, v8::Context>,
    host_functions: &[HostFunctionDef],
    scripts: &[Source],
) -> Result<(), EngineError> {
    let shared = Shared::default();
    let mut names = Vec::with_capacity(host_functions.len());
    for def in host_functions {
        host::install(scope, &shared, *def)?;
        names.push(js_string(scope, def.name)?.into());
    }
    let names = v8::Array::new_with_elements(scope, &names);
    scope.add_context_data(context, names);
    let null = v8::null(scope).into();
    let results = v8::Object::with_prototype_and_properties(scope, null, &[], &[]);
    let key = js_string(scope, SNAPSHOT_RESULTS_KEY)?;
    let key = v8::Symbol::for_key(scope, key);
    context.global(scope).set(scope, key.into(), results.into());
    for source in scripts {
        let value = run(scope, source)?;
        if !value.is_undefined() {
            let name = js_string(scope, &source.name)?;
            results.set(scope, name.into(), value);
        }
    }
    Ok(())
}

/// Binds `host_functions` to the op list of the snapshot the current context came from: the
/// functions in the heap call by index, so a list that differs in names or order would run the
/// wrong op with the isolate's authority.
pub(crate) fn relink(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    host_functions: &[HostFunctionDef],
) -> Result<(), EngineError> {
    let names = scope
        .get_context_data_from_snapshot_once::<v8::Array>(OP_NAMES)
        .map_err(|_| VelyrError::invalid_data("the snapshot has no host function list"))?;
    let baked: Vec<String> = (0..names.length())
        .map(|index| {
            names
                .get_index(scope, index)
                .map_or_else(String::new, |name| name.to_rust_string_lossy(scope))
        })
        .collect();
    let given = host_functions.iter().map(|def| def.name);
    if !baked.iter().map(String::as_str).eq(given) {
        return Err(VelyrError::invalid_data(format!(
            "the snapshot's host function list ({} ops) differs from the one given ({} ops)",
            baked.len(),
            host_functions.len()
        ))
        .into());
    }
    let mut functions = shared.host_fns.borrow_mut();
    let mut indexes = shared.host_names.borrow_mut();
    for (index, def) in host_functions.iter().enumerate() {
        functions.push(def.callback);
        indexes.insert(def.name, index);
    }
    Ok(())
}

fn js_string<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    text: &str,
) -> Result<v8::Local<'s, v8::String>, VelyrError> {
    v8::String::new(scope, text).ok_or_else(|| VelyrError::internal("snapshot string too long"))
}

fn run<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    source: &Source,
) -> Result<v8::Local<'s, v8::Value>, EngineError> {
    v8::tc_scope!(let tc, scope);
    let ran = v8::String::new(tc, &source.name)
        .zip(v8::String::new(tc, &source.text))
        .and_then(|(name, text)| {
            let origin = script_origin(tc, name);
            v8::Script::compile(tc, text, Some(&origin))
        })
        .and_then(|script| script.run(tc));
    if let Some(value) = ran {
        return Ok(value);
    }
    let message = tc
        .exception()
        .map_or_else(|| "source too long".into(), |e| e.to_rust_string_lossy(tc));
    Err(VelyrError::internal(format!(
        "snapshot script `{}` failed: {message}",
        source.name
    ))
    .into())
}

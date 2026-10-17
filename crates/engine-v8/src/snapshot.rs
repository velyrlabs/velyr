//! Startup snapshots (VELYR-123): a context with bootstrap scripts already evaluated, so an
//! isolate booted from it (see [`crate::V8Engine::with_snapshot`]) skips that work.

use std::borrow::Cow;

use v8::MapFnTo;
use velyr_engine_api::{EngineError, Source};
use velyr_runtime_errors::VelyrError;

use crate::{V8Engine, host, modules};

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

/// Runs `scripts` in order in a fresh context and serializes it as the snapshot's default
/// context. The scripts must be pure JS: no host function is registered while they run.
///
/// Meant for build scripts: run it alone in its process. V8 crashes or corrupts heaps when it
/// runs concurrently with another snapshot creation or with isolates booted from a snapshot.
pub fn create_snapshot(scripts: &[Source]) -> Result<Vec<u8>, EngineError> {
    V8Engine::init_platform();
    let mut isolate = v8::Isolate::snapshot_creator(Some(external_references()), None);
    let ran = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, Default::default());
        let scope = &mut v8::ContextScope::new(scope, context);
        scope.set_default_context(context);
        scripts.iter().try_for_each(|source| run(scope, source))
    };
    // A snapshot creator must reach `create_blob` even on failure: dropping it panics.
    let blob = isolate.create_blob(v8::FunctionCodeHandling::Keep);
    ran?;
    blob.map(|blob| blob.to_vec())
        .ok_or_else(|| VelyrError::internal("V8 could not create the snapshot blob").into())
}

fn run(scope: &mut v8::PinScope<'_, '_>, source: &Source) -> Result<(), EngineError> {
    v8::tc_scope!(let tc, scope);
    let ran = v8::String::new(tc, &source.text)
        .and_then(|text| v8::Script::compile(tc, text, None))
        .and_then(|script| script.run(tc));
    if ran.is_some() {
        return Ok(());
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

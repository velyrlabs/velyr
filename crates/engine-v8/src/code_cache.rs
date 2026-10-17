//! V8 code cache for modules (VELYR-60, experimental): consume on compile, produce on demand.
//!
//! Blobs are sealed with `velyr_engine_api::seal_code_cache` so V8 only sees bytes that match
//! the exact source text and are intact; V8 itself checks little more than the source length.

use velyr_engine_api::{
    JsException, Source, Value, function_digest, open_code_cache, seal_code_cache,
};

use crate::convert::{exception_from, from_v8, internal_exception};
use crate::shared::Shared;

/// A function made by `compile_function`, kept so its cache can be created after it ran.
pub(crate) struct CompiledFunction {
    function: v8::Global<v8::Function>,
    text: v8::Global<v8::String>,
    params: Vec<String>,
    /// Known when a cache was consumed; otherwise computed by [`create_function`].
    digest: Option<u64>,
}

/// `source_digest` of `text`; every digest this crate computes goes through here.
pub(crate) fn digest(text: &str) -> u64 {
    #[cfg(test)]
    tests::DIGESTS.with(|n| n.set(n.get() + 1));
    velyr_engine_api::source_digest(text)
}

/// The V8 payload inside a sealed `blob`, if it is valid for the source with this `digest`.
pub(crate) fn unseal(digest: u64, blob: &[u8]) -> Option<&[u8]> {
    open_code_cache(digest, blob)
}

/// Records a consumed cache as accepted or rejected.
pub(crate) fn record(shared: &Shared, accepted: bool) {
    let mut stats = shared.code_cache_stats.get();
    if accepted {
        stats.accepted += 1;
    } else {
        stats.rejected += 1;
    }
    shared.code_cache_stats.set(stats);
}

/// Sealed cache of module `name`, or `None` if the module is unknown or V8 produced nothing.
pub(crate) fn create(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    name: &str,
) -> Option<Vec<u8>> {
    let (module, known, text) = {
        let graph = shared.modules.borrow();
        let module = v8::Local::new(scope, graph.module(name)?);
        let text = graph.text(name).map(|text| v8::Local::new(scope, text));
        (module, graph.digest(name), text)
    };
    let digest = match known {
        Some(digest) => digest,
        None => {
            let digest = digest(&text?.to_rust_string_lossy(scope));
            shared.modules.borrow_mut().remember_digest(name, digest);
            digest
        }
    };
    let payload = module
        .get_unbound_module_script(scope)
        .create_code_cache()?;
    Some(seal_code_cache(digest, &payload))
}

/// Compiles `source` as the body of a function of `params`, consuming `cache` when valid.
pub(crate) fn compile_function(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    source: &Source,
    params: &[&str],
    cache: Option<&[u8]>,
) -> Result<Value, JsException> {
    let digest = cache.map(|_| function_digest(params, &source.text));
    let payload = cache.zip(digest).map(|(blob, digest)| unseal(digest, blob));
    v8::tc_scope!(let tc, scope);
    let (Some(name), Some(text)) = (
        v8::String::new(tc, &source.name),
        v8::String::new(tc, &source.text),
    ) else {
        return Err(internal_exception("source too long"));
    };
    let arguments: Option<Vec<_>> = params.iter().map(|p| v8::String::new(tc, p)).collect();
    let Some(arguments) = arguments else {
        return Err(internal_exception("parameter list too long"));
    };
    let origin = crate::script_origin(tc, name);
    let mut v8_source = match payload.flatten() {
        Some(bytes) => v8::script_compiler::Source::new_with_cached_data(
            text,
            Some(&origin),
            v8::script_compiler::CachedData::new(bytes),
        ),
        None => v8::script_compiler::Source::new(text, Some(&origin)),
    };
    let options = if v8_source.get_cached_data().is_some() {
        v8::script_compiler::CompileOptions::ConsumeCodeCache
    } else {
        v8::script_compiler::CompileOptions::NoCompileOptions
    };
    let compiled = v8::script_compiler::compile_function(
        tc,
        &mut v8_source,
        &arguments,
        &[],
        options,
        v8::script_compiler::NoCacheReason::NoReason,
    );
    if cache.is_some() {
        let accepted = v8_source
            .get_cached_data()
            .is_some_and(|data| !data.rejected());
        record(shared, accepted);
    }
    let Some(function) = compiled else {
        return Err(exception_from(tc, shared));
    };
    shared.functions.borrow_mut().insert(
        source.name.clone(),
        CompiledFunction {
            function: v8::Global::new(tc, function),
            text: v8::Global::new(tc, text),
            params: params.iter().map(|p| (*p).to_owned()).collect(),
            digest,
        },
    );
    Ok(from_v8(
        tc,
        &mut shared.handles.borrow_mut(),
        function.into(),
    ))
}

/// Sealed cache of the function compiled as `name`, or `None` if unknown or V8 produced nothing.
pub(crate) fn create_function(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    name: &str,
) -> Option<Vec<u8>> {
    let (function, text, params, known) = {
        let functions = shared.functions.borrow();
        let entry = functions.get(name)?;
        (
            v8::Local::new(scope, &entry.function),
            v8::Local::new(scope, &entry.text),
            entry.params.clone(),
            entry.digest,
        )
    };
    let digest = known.unwrap_or_else(|| {
        let params: Vec<&str> = params.iter().map(String::as_str).collect();
        function_digest(&params, &text.to_rust_string_lossy(scope))
    });
    let payload = function.create_code_cache()?;
    Some(seal_code_cache(digest, &payload))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source};

    use crate::V8Engine;

    thread_local! {
        pub(super) static DIGESTS: Cell<usize> = const { Cell::new(0) };
    }

    #[test]
    fn digest_is_computed_only_when_a_code_cache_is_involved() {
        let mut isolate = V8Engine::new()
            .create_isolate(IsolateOptions::default())
            .unwrap();
        let source = Source {
            name: "m.js".into(),
            text: "export const a = 1;".into(),
        };
        let before = DIGESTS.with(Cell::get);
        isolate.compile_module(&source).unwrap();
        assert_eq!(DIGESTS.with(Cell::get), before);
        let blob = isolate.create_code_cache("m.js").expect("cache produced");
        assert_eq!(DIGESTS.with(Cell::get), before + 1);
        let again = Source {
            name: "n.js".into(),
            ..source
        };
        isolate.compile_module_cached(&again, Some(&blob)).unwrap();
        assert_eq!(DIGESTS.with(Cell::get), before + 2);
    }
}

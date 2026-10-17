//! V8 code cache for modules (VELYR-60, experimental): consume on compile, produce on demand.
//!
//! Blobs are sealed with `velyr_engine_api::seal_code_cache` so V8 only sees bytes that match
//! the exact source text and are intact; V8 itself checks little more than the source length.

use velyr_engine_api::{open_code_cache, seal_code_cache};

use crate::shared::Shared;

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

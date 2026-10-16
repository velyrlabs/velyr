//! V8 code cache for modules (VELYR-60, experimental): consume on compile, produce on demand.
//!
//! Blobs are sealed with `velyr_engine_api::seal_code_cache` so V8 only sees bytes that match
//! the exact source text and are intact; V8 itself checks little more than the source length.

use velyr_engine_api::{open_code_cache, seal_code_cache};

use crate::shared::Shared;

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
    let (module, digest) = {
        let graph = shared.modules.borrow();
        let module = v8::Local::new(scope, graph.module(name)?);
        (module, graph.digest(name)?)
    };
    let payload = module
        .get_unbound_module_script(scope)
        .create_code_cache()?;
    Some(seal_code_cache(digest, &payload))
}

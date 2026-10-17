//! Glue between the shared code cache layer and the engine (VELYR-60, experimental).

use std::sync::Arc;

use velyr_engine_api::Isolate;
use velyr_runtime_modules::{ModuleId, SharedCache};

use super::sync::SourceCache;

/// Publishes the code cache of every module this isolate compiled and the shared layer lacks.
/// Called after the entry module's top-level ran, so the blobs include lazily compiled functions.
pub(super) fn publish(isolate: &mut impl Isolate, shared: &SharedCache, loaded: &SourceCache) {
    let entries: Vec<(ModuleId, String)> = loaded
        .lock()
        .unwrap()
        .iter()
        .map(|(id, source)| (id.clone(), source.hash.clone()))
        .collect();
    let mut produced = 0;
    for (id, hash) in entries {
        if shared.code(&id, &hash).is_some() {
            continue;
        }
        if let Some(blob) = isolate.create_code_cache(id.as_str()) {
            shared.insert_code(&id, &hash, Arc::from(blob));
            produced += 1;
        }
    }
    for (id, hash) in shared.take_pending_functions() {
        let blob = id
            .to_path()
            .and_then(|path| isolate.create_function_code_cache(&path.to_string_lossy()));
        if let Some(blob) = blob {
            shared.insert_code(&id, &hash, Arc::from(blob));
            produced += 1;
        }
    }
    let stats = isolate.code_cache_stats();
    shared.record_uses(stats.accepted, stats.rejected);
    tracing::debug!(
        accepted = stats.accepted,
        rejected = stats.rejected,
        produced,
        total_bytes = shared.code_bytes(),
        "velyr.core.code_cache"
    );
}

//! `ModuleResolver` wrapper adding `velyr.json` imports, tsconfig `paths` and `baseUrl`.
//!
//! Every candidate generated here is authorized before the inner resolver can stat it, so the
//! existence of files outside the grant is not observable through this wrapper.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_modules::{
    ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result, authorize_path,
};
use velyr_runtime_permissions::CapabilitySet;

use crate::config::{Paths, normalize};
use crate::{TsConfig, TsconfigLoader};

const HELPERS_PREFIX: &str = "velyr-oxc-runtime/";

/// `imports` of a `velyr.json`; relative values resolve against `dir`.
#[derive(Debug, Clone, Default)]
pub struct ImportMap {
    pub dir: PathBuf,
    pub entries: HashMap<String, String>,
}

pub struct ProjectResolver {
    inner: Arc<dyn ModuleResolver + Send + Sync>,
    loader: Arc<TsconfigLoader>,
    permissions: Arc<CapabilitySet>,
    imports: Option<ImportMap>,
}

impl ProjectResolver {
    pub fn new(
        inner: Arc<dyn ModuleResolver + Send + Sync>,
        loader: Arc<TsconfigLoader>,
        permissions: Arc<CapabilitySet>,
        imports: Option<ImportMap>,
    ) -> Self {
        Self {
            inner,
            loader,
            permissions,
            imports,
        }
    }

    /// The caller's context, with the import map replaced only when this wrapper owns one.
    fn derive(&self, ctx: &ResolveContext) -> ResolveContext {
        match &self.imports {
            Some(map) => ResolveContext {
                import_map: map.entries.clone(),
                import_map_dir: map.dir.clone(),
                ..ctx.clone()
            },
            None => ctx.clone(),
        }
    }

    fn try_candidate(
        &self,
        candidate: &Path,
        referrer: &ModuleId,
        derived: &ResolveContext,
    ) -> Result<Option<ResolvedModule>> {
        let spec = candidate.to_string_lossy();
        match self.inner.resolve(&spec, referrer, derived) {
            Ok(found) => Ok(Some(found)),
            Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn resolve_paths(
        &self,
        specifier: &str,
        paths: &Paths,
        referrer: &ModuleId,
        derived: &ResolveContext,
    ) -> Result<Option<ResolvedModule>> {
        let Some((pattern, targets, captured)) = match_pattern(specifier, paths) else {
            return Ok(None);
        };
        let mut skipped = 0;
        let mut tried = 0;
        for target in targets.iter().filter(|t| !t.ends_with(".d.ts")) {
            tried += 1;
            let candidate = normalize(&paths.base.join(target.replacen('*', captured, 1)));
            if authorize_path(&candidate, &self.permissions).is_err() {
                skipped += 1;
                continue;
            }
            if let Some(found) = self.try_candidate(&candidate, referrer, derived)? {
                return Ok(Some(found));
            }
        }
        if tried > 0 && skipped == tried {
            return Err(VelyrError::not_found(format!(
                "module not found: {specifier}: matched tsconfig paths pattern `{pattern}` but \
                 every target is outside the permitted roots"
            )));
        }
        Ok(None)
    }

    fn resolve_base_url(
        &self,
        specifier: &str,
        config: &TsConfig,
        referrer: &ModuleId,
        derived: &ResolveContext,
    ) -> Result<Option<ResolvedModule>> {
        let Some(base_url) = config.options.base_url.as_ref() else {
            return Ok(None);
        };
        let candidate = normalize(&base_url.join(specifier));
        if authorize_path(&candidate, &self.permissions).is_err() {
            return Ok(None);
        }
        self.try_candidate(&candidate, referrer, derived)
    }
}

impl ModuleResolver for ProjectResolver {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleId,
        ctx: &ResolveContext,
    ) -> Result<ResolvedModule> {
        let derived = self.derive(ctx);
        if is_passthrough(specifier) || matches_import_map(specifier, &derived) {
            return self.inner.resolve(specifier, referrer, &derived);
        }
        let config = match referrer.to_path() {
            Some(importer) => self.loader.for_file(&importer)?,
            None => None,
        };
        if let Some(config) = config {
            if let Some(paths) = config.options.paths.as_ref()
                && let Some(found) = self.resolve_paths(specifier, paths, referrer, &derived)?
            {
                return Ok(found);
            }
            if !specifier.contains(':')
                && let Some(found) =
                    self.resolve_base_url(specifier, &config, referrer, &derived)?
            {
                return Ok(found);
            }
        }
        self.inner.resolve(specifier, referrer, &derived)
    }
}

fn is_passthrough(specifier: &str) -> bool {
    specifier.starts_with(['.', '/'])
        || specifier.contains(':')
        || specifier.starts_with(HELPERS_PREFIX)
}

/// Same matching as the inner resolver: exact key, else a key ending in `/` that prefixes it.
fn matches_import_map(specifier: &str, ctx: &ResolveContext) -> bool {
    ctx.import_map.contains_key(specifier)
        || ctx
            .import_map
            .keys()
            .any(|key| key.ends_with('/') && specifier.starts_with(key.as_str()))
}

/// Exact key first, else the pattern with one `*` and the longest prefix.
fn match_pattern<'a>(
    specifier: &'a str,
    paths: &'a Paths,
) -> Option<(&'a str, &'a [String], &'a str)> {
    if let Some((key, targets)) = paths.map.get_key_value(specifier) {
        return Some((key, targets, ""));
    }
    paths
        .map
        .iter()
        .filter_map(|(key, targets)| {
            let (prefix, suffix) = key.split_once('*')?;
            let rest = specifier.strip_prefix(prefix)?.strip_suffix(suffix)?;
            Some((prefix.len(), key.as_str(), targets.as_slice(), rest))
        })
        .max_by_key(|(prefix_len, ..)| *prefix_len)
        .map(|(_, key, targets, rest)| (key, targets, rest))
}

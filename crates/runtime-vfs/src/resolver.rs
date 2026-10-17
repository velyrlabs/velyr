use std::path::PathBuf;
use std::sync::Arc;

use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result};

use crate::Vfs;

/// Resolves imports from the edges recorded at compile time. Specifiers without an edge (the
/// `node:` facades) go to `fallback`; a module that is not embedded is never searched on disk.
pub struct VfsResolver {
    vfs: Arc<Vfs>,
    fallback: Option<Arc<dyn ModuleResolver + Send + Sync>>,
}

impl VfsResolver {
    pub fn new(vfs: Arc<Vfs>, fallback: Option<Arc<dyn ModuleResolver + Send + Sync>>) -> Self {
        Self { vfs, fallback }
    }

    fn found(path: &str) -> Result<ResolvedModule> {
        let path = PathBuf::from(path);
        let id = ModuleId::from_path(&path)
            .ok_or_else(|| VelyrError::invalid_data("embedded module path is not absolute"))?;
        Ok(ResolvedModule { id, path })
    }
}

impl ModuleResolver for VfsResolver {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleId,
        ctx: &ResolveContext,
    ) -> Result<ResolvedModule> {
        let from = referrer.to_path().map(|p| p.to_string_lossy().into_owned());
        if let Some(target) = from.and_then(|from| self.vfs.edge(&from, specifier)) {
            return Self::found(target);
        }
        if self.vfs.exists(specifier) {
            return Self::found(specifier);
        }
        match &self.fallback {
            Some(fallback) => fallback.resolve(specifier, referrer, ctx),
            None => Err(VelyrError::not_found(format!(
                "module not embedded: {specifier}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use velyr_runtime_errors::ErrorCode;

    use super::*;
    use crate::{Payload, edge_key};

    fn resolver() -> VfsResolver {
        let payload = Payload {
            files: BTreeMap::from([("/$velyr/app/a.js".to_owned(), Vec::new())]),
            edges: BTreeMap::from([(
                edge_key("/$velyr/app/main.js", "./a.js"),
                "/$velyr/app/a.js".to_owned(),
            )]),
            ..Payload::default()
        };
        VfsResolver::new(Arc::new(Vfs::from_payload(payload)), None)
    }

    #[test]
    fn follows_recorded_edges_and_absolute_embedded_paths() {
        let main = ModuleId::from_path(&PathBuf::from("/$velyr/app/main.js")).unwrap();
        let ctx = ResolveContext::default();
        let by_edge = resolver().resolve("./a.js", &main, &ctx).unwrap();
        assert_eq!(by_edge.path, PathBuf::from("/$velyr/app/a.js"));
        let by_path = resolver().resolve("/$velyr/app/a.js", &main, &ctx).unwrap();
        assert_eq!(by_path.id, by_edge.id);
    }

    #[test]
    fn unknown_specifier_is_not_found_and_never_touches_disk() {
        let main = ModuleId::from_path(&PathBuf::from("/$velyr/app/main.js")).unwrap();
        let err = resolver()
            .resolve("./b.js", &main, &ResolveContext::default())
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);
    }
}

use std::sync::Arc;

use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{
    BoxFuture, LoadContext, ModuleLoader, ModuleSource, ResolvedModule, Result,
};

use crate::Vfs;

/// Serves modules from the embedded payload instead of the disk. Sources are already
/// transformed at compile time, so no transform runs on top of this loader.
pub struct VfsLoader(pub Arc<Vfs>);

impl ModuleLoader for VfsLoader {
    fn load<'a>(
        &'a self,
        module: &'a ResolvedModule,
        _ctx: &'a LoadContext,
    ) -> BoxFuture<'a, Result<ModuleSource>> {
        Box::pin(async move {
            let key = module.path.to_string_lossy();
            let bytes = self
                .0
                .read(&key)
                .ok_or_else(|| VelyrError::not_found("module not found"))?;
            let text = std::str::from_utf8(bytes)
                .map_err(|_| VelyrError::invalid_data("module is not readable UTF-8 text"))?;
            Ok(ModuleSource::new(text.to_owned()))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use velyr_runtime_modules::ModuleId;
    use velyr_runtime_permissions::CapabilitySet;

    use super::*;

    #[tokio::test]
    async fn loads_embedded_module_and_reports_missing() {
        let vfs = Vfs::new(BTreeMap::from([("/app/a.js".to_owned(), b"export {}".to_vec())]));
        let loader = VfsLoader(Arc::new(vfs));
        let ctx = LoadContext { permissions: CapabilitySet::builder().build() };
        let at = |p: &str| ResolvedModule { id: ModuleId::from_path(&PathBuf::from(p)).unwrap(), path: PathBuf::from(p) };

        assert_eq!(loader.load(&at("/app/a.js"), &ctx).await.unwrap().text, "export {}");
        assert!(loader.load(&at("/app/b.js"), &ctx).await.is_err());
    }
}

//! Source transforms the module host applies to every module, in every profile but `minimal`.

use std::sync::Arc;

use velyr_engine_api::EngineFeatures;
use velyr_runtime_core::ModuleResolverHandle;
use velyr_runtime_modules::{MediaType, ModuleId, Result, SourceTransform, Transformed};
use velyr_runtime_permissions::CapabilitySet;

/// The module host's transform: TypeScript is stripped first, then CommonJS and JSON files get
/// their ESM facade (node and full only). `None` in `profile-minimal`, which has neither.
/// `features` are the engine's: syntax it lacks (`using`) is lowered. `permissions` gate the files the CommonJS analysis follows through `module.exports = require(..)`.
pub fn source_transform(
    permissions: &Arc<CapabilitySet>,
    features: EngineFeatures,
) -> Option<Arc<dyn SourceTransform>> {
    #[cfg(not(feature = "node"))]
    let _ = permissions;
    let mut chain = strip_steps(permissions, features);
    #[cfg(feature = "node")]
    chain.push(Arc::new(velyr_compat_commonjs::CommonJsTransform::new(
        permissions.clone(),
    )));
    match chain.len() {
        0 => None,
        1 => chain.pop(),
        _ => Some(Arc::new(Chain(chain))),
    }
}

/// JSX and TypeScript steps, without the CommonJS facade.
fn strip_steps(
    permissions: &Arc<CapabilitySet>,
    features: EngineFeatures,
) -> Vec<Arc<dyn SourceTransform>> {
    #[cfg(not(any(feature = "typescript", feature = "jsx")))]
    let _ = permissions;
    #[cfg(not(feature = "typescript"))]
    let _ = features;
    let mut chain: Vec<Arc<dyn SourceTransform>> = Vec::new();
    // Before TypeScript: it rewrites `.tsx` (types and JSX) in one pass, the TypeScript step then
    // sees plain JavaScript.
    #[cfg(any(feature = "typescript", feature = "jsx"))]
    let loader = Arc::new(velyr_tooling_tsconfig::TsconfigLoader::new(
        permissions.clone(),
    ));
    #[cfg(feature = "jsx")]
    chain.push(Arc::new(
        velyr_tooling_jsx::JsxTransform::default()
            .with_tsconfig(loader.clone())
            .with_overrides(ProjectConfig::current().jsx.clone()),
    ));
    #[cfg(feature = "typescript")]
    chain.push(Arc::new(
        velyr_tooling_typescript::TypeScriptTransform::with_platform_cache()
            .with_tsconfig(loader)
            .with_lowering(velyr_tooling_typescript::Lowering {
                explicit_resource_management: !features
                    .contains(EngineFeatures::EXPLICIT_RESOURCE_MANAGEMENT),
            }),
    ));
    chain
}

/// What `require` runs on the TypeScript it reads: the steps of [`source_transform`] without the
/// CommonJS facade (which would recurse). `None` when neither TypeScript nor JSX is built in.
pub fn commonjs_strip(
    permissions: &Arc<CapabilitySet>,
    features: EngineFeatures,
) -> Option<Arc<dyn SourceTransform>> {
    let mut chain = strip_steps(permissions, features);
    match chain.len() {
        0 => None,
        1 => chain.pop(),
        _ => Some(Arc::new(Chain(chain))),
    }
}

/// What `velyr.json` contributes to module loading: the import map and the `jsx*` overrides.
#[cfg(any(feature = "typescript", feature = "jsx"))]
#[derive(Default)]
struct ProjectConfig {
    import_map: Option<velyr_tooling_tsconfig::ImportMap>,
    jsx: velyr_tooling_jsx::JsxOverrides,
}

#[cfg(any(feature = "typescript", feature = "jsx"))]
impl ProjectConfig {
    fn from_loaded(loaded: crate::config::Loaded) -> Self {
        let crate::config::Loaded { dir, config } = loaded;
        let import_map = config
            .imports
            .filter(|entries| !entries.is_empty())
            .map(|entries| velyr_tooling_tsconfig::ImportMap {
                dir,
                entries: entries.into_iter().collect(),
            });
        let options = config.compiler_options.unwrap_or_default();
        let jsx = velyr_tooling_jsx::JsxOverrides {
            jsx: options.jsx,
            import_source: options.jsx_import_source,
            factory: options.jsx_factory,
            fragment_factory: options.jsx_fragment_factory,
        };
        Self { import_map, jsx }
    }

    /// Read once from the `velyr.json` above the working directory; a broken one is reported and
    /// ignored (the transform hook cannot fail).
    fn current() -> &'static Self {
        static CONFIG: std::sync::OnceLock<ProjectConfig> = std::sync::OnceLock::new();
        CONFIG.get_or_init(|| {
            let cwd = std::env::current_dir().unwrap_or_default();
            match crate::config::discover(&cwd) {
                Ok(loaded) => loaded.map(Self::from_loaded).unwrap_or_default(),
                Err(error) => {
                    eprintln!("warning: ignoring velyr.json: {error}");
                    Self::default()
                }
            }
        })
    }
}

/// Wraps the module resolver with tsconfig `paths`/`baseUrl` and the `velyr.json` import map.
/// Without a resolver the wrapper sits on the plain filesystem one.
pub fn project_resolver(
    inner: Option<ModuleResolverHandle>,
    permissions: &Arc<CapabilitySet>,
) -> Option<ModuleResolverHandle> {
    #[cfg(any(feature = "typescript", feature = "jsx"))]
    return project_resolver_for(ProjectConfig::current(), inner, permissions);
    #[cfg(not(any(feature = "typescript", feature = "jsx")))]
    {
        let _ = permissions;
        inner
    }
}

#[cfg(any(feature = "typescript", feature = "jsx"))]
fn project_resolver_for(
    config: &ProjectConfig,
    inner: Option<ModuleResolverHandle>,
    permissions: &Arc<CapabilitySet>,
) -> Option<ModuleResolverHandle> {
    let inner = inner.unwrap_or_else(|| Arc::new(velyr_runtime_modules::FsResolver));
    let loader = Arc::new(velyr_tooling_tsconfig::TsconfigLoader::new(
        permissions.clone(),
    ));
    Some(Arc::new(velyr_tooling_tsconfig::ProjectResolver::new(
        inner,
        loader,
        permissions.clone(),
        config.import_map.clone(),
    )))
}

/// Runs each transform in order on the previous one's output.
struct Chain(Vec<Arc<dyn SourceTransform>>);

impl SourceTransform for Chain {
    fn handles(&self, media_type: MediaType) -> bool {
        self.0.iter().any(|step| step.handles(media_type))
    }

    fn applies(&self, id: &ModuleId, text: &str) -> bool {
        let media_type = id
            .to_path()
            .map_or(MediaType::JavaScript, |p| MediaType::from_path(&p));
        self.0
            .iter()
            .any(|step| step.handles(media_type) && step.applies(id, text))
    }

    fn transform(
        &self,
        specifier: &str,
        source: &str,
        media_type: MediaType,
    ) -> Result<Transformed> {
        let mut out = Transformed {
            code: source.to_owned(),
            source_map: None,
        };
        for step in self.0.iter().filter(|step| step.handles(media_type)) {
            let next = step.transform(specifier, &out.code, media_type)?;
            // The first map stays: a later step that leaves the text alone must not lose it.
            out = Transformed {
                source_map: out.source_map.or(next.source_map),
                ..next
            };
        }
        Ok(out)
    }
}

#[cfg(all(test, feature = "typescript"))]
mod tests {
    use super::*;

    #[test]
    fn strips_types_and_leaves_javascript_alone() {
        let transform = source_transform(&Arc::default(), EngineFeatures::empty()).unwrap();
        assert!(transform.handles(MediaType::TypeScript));
        // Plain ESM passes through unchanged (with CommonJS in the chain it is still visited).
        let js = transform
            .transform("/m.mjs", "export const a = 1;", MediaType::JavaScript)
            .unwrap();
        assert_eq!(js.code, "export const a = 1;");
        let out = transform
            .transform("/m.ts", "const a: number = 1;", MediaType::TypeScript)
            .unwrap();
        assert!(!out.code.contains("number"), "{}", out.code);
    }

    fn allow(dir: &std::path::Path) -> Arc<CapabilitySet> {
        use velyr_runtime_permissions::{Capability, PathPattern};
        CapabilitySet::builder()
            .allow(Capability::ImportLocal(PathPattern::new(
                dir.to_str().unwrap(),
            )))
            .build()
    }

    fn write(root: &std::path::Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn tsconfig_is_found_from_the_file_not_the_cwd() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "sub/tsconfig.json",
            r#"{ "compilerOptions": { "experimentalDecorators": true } }"#,
        );
        let file = dir.path().join("sub/a.ts");
        let transform = source_transform(&allow(dir.path()), EngineFeatures::empty()).unwrap();
        let out = transform
            .transform(
                file.to_str().unwrap(),
                "function D() { return () => {}; }\n@D() class A {}\n",
                MediaType::TypeScript,
            )
            .unwrap();
        assert!(
            out.code.contains("velyr-oxc-runtime/helpers/decorate"),
            "{}",
            out.code
        );
    }

    #[test]
    fn project_resolver_applies_velyr_json_imports() {
        use velyr_runtime_modules::ResolveContext;
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        write(&root, "velyr.json", r#"{ "imports": { "~/": "./src/" } }"#);
        write(&root, "src/a.ts", "export const a = 1;");
        write(&root, "src/main.ts", "import '~/a';");
        let loaded = crate::config::discover(&root).unwrap().unwrap();
        let config = ProjectConfig::from_loaded(loaded);
        let resolver = project_resolver_for(&config, None, &allow(&root)).unwrap();
        let referrer = ModuleId::from_path(&root.join("src/main.ts")).unwrap();
        let found = resolver
            .resolve("~/a", &referrer, &ResolveContext::default())
            .unwrap();
        assert_eq!(found.path, root.join("src/a.ts"));
    }
}

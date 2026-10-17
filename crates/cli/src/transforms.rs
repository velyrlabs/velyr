//! Source transforms the module host applies to every module, in every profile but `minimal`.

use std::sync::Arc;

use velyr_engine_api::EngineFeatures;
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
    #[cfg(not(feature = "typescript"))]
    let _ = features;
    let mut chain: Vec<Arc<dyn SourceTransform>> = Vec::new();
    // Before TypeScript: it rewrites `.tsx` (types and JSX) in one pass, the TypeScript step then
    // sees plain JavaScript.
    #[cfg(feature = "jsx")]
    chain.push(Arc::new(jsx_transform()));
    #[cfg(feature = "typescript")]
    chain.push(Arc::new(
        velyr_tooling_typescript::TypeScriptTransform::with_platform_cache().with_lowering(
            velyr_tooling_typescript::Lowering {
                explicit_resource_management: !features
                    .contains(EngineFeatures::EXPLICIT_RESOURCE_MANAGEMENT),
            },
        ),
    ));
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

/// JSX settings from the `velyr.json`/`tsconfig.json` above the working directory; a broken config
/// is reported and the defaults apply (this hook cannot fail).
#[cfg(feature = "jsx")]
fn jsx_transform() -> velyr_tooling_jsx::JsxTransform {
    let dir = std::env::current_dir().unwrap_or_default();
    velyr_tooling_jsx::JsxTransform::discover(&dir).unwrap_or_else(|error| {
        eprintln!("warning: ignoring JSX config: {error}");
        velyr_tooling_jsx::JsxTransform::default()
    })
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
}

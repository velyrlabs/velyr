use std::path::PathBuf;
use std::sync::Arc;

use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_module_file};
use velyr_runtime_modules::{
    ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result as ModuleResult,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

/// Maps the bare specifier `dep` to a fixed file.
struct Alias(PathBuf);

impl ModuleResolver for Alias {
    fn resolve(&self, _: &str, _: &ModuleId, _: &ResolveContext) -> ModuleResult<ResolvedModule> {
        Ok(ResolvedModule {
            id: ModuleId::from_path(&self.0).expect("absolute path"),
            path: self.0.clone(),
        })
    }
}

fn run(resolver: Option<Arc<dyn ModuleResolver + Send + Sync>>) -> Result<(), RunError> {
    let dir = dunce::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("velyr-resolver-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dep.mjs"), "export const ok = true;").unwrap();
    std::fs::write(
        dir.join("main.mjs"),
        "import { ok } from 'dep'; if (!ok) throw 1;",
    )
    .unwrap();
    let capabilities = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            dir.to_str().unwrap(),
        )))
        .build();
    let config = RuntimeConfig {
        capabilities: Some(capabilities),
        platform_init: Some(V8Engine::init_platform),
        module_resolver: resolver,
        ..RuntimeConfig::default()
    };
    run_module_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        |_, _, _| {},
        dir.join("main.mjs"),
    )
}

#[test]
fn bare_specifier_fails_with_the_default_resolver() {
    assert!(matches!(run(None), Err(RunError::Load(_))));
}

#[test]
fn a_configured_resolver_serves_the_imports() {
    let dep = dunce::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("velyr-resolver-{}", std::process::id()))
        .join("dep.mjs");
    // `run` writes dep.mjs; make sure it exists before the resolver is asked.
    std::fs::create_dir_all(dep.parent().unwrap()).unwrap();
    std::fs::write(&dep, "export const ok = true;").unwrap();
    assert!(run(Some(Arc::new(Alias(dep)))).is_ok());
}

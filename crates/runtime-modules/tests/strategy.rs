//! The bare-specifier hook: `StrategyResolver` delegates what the import map leaves bare.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_modules::{
    BareSpecifierResolver, FsLoader, LoadContext, ModuleHost, ModuleId, ModuleResolver,
    ResolveContext, Result, StrategyResolver,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

/// Maps every bare specifier to a fixed target and records what it was asked.
struct Fixed {
    target: Option<PathBuf>,
    calls: Mutex<Vec<(String, PathBuf)>>,
}

impl BareSpecifierResolver for Fixed {
    fn resolve_bare(&self, specifier: &str, referrer: &Path) -> Result<PathBuf> {
        self.calls
            .lock()
            .unwrap()
            .push((specifier.to_owned(), referrer.to_owned()));
        self.target
            .clone()
            .ok_or_else(|| VelyrError::not_found("strategy says no"))
    }
}

fn project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    for f in ["main.js", "a.js", "node_modules/pkg/index.js"] {
        fs::write(root.join(f), "export {};").unwrap();
    }
    (dir, root)
}

fn setup(target: Option<PathBuf>) -> (Arc<Fixed>, StrategyResolver) {
    let fixed = Arc::new(Fixed {
        target,
        calls: Mutex::default(),
    });
    let resolver = StrategyResolver::new(fixed.clone());
    (fixed, resolver)
}

fn referrer(root: &Path) -> ModuleId {
    ModuleId::from_path(&root.join("main.js")).unwrap()
}

#[test]
fn bare_specifier_is_delegated_and_canonicalized() {
    let (_d, root) = project();
    let (fixed, resolver) = setup(Some(root.join("node_modules/pkg/../pkg/index.js")));
    let got = resolver
        .resolve("pkg", &referrer(&root), &ResolveContext::default())
        .unwrap();
    assert_eq!(got.path, root.join("node_modules/pkg/index.js"));
    let calls = fixed.calls.lock().unwrap();
    assert_eq!(*calls, vec![("pkg".to_owned(), root.join("main.js"))]);
}

#[test]
fn hash_imports_are_delegated_too() {
    let (_d, root) = project();
    let (fixed, resolver) = setup(Some(root.join("a.js")));
    resolver
        .resolve("#internal", &referrer(&root), &ResolveContext::default())
        .unwrap();
    assert_eq!(fixed.calls.lock().unwrap()[0].0, "#internal");
}

#[test]
fn relative_and_import_map_hits_skip_the_strategy() {
    let (_d, root) = project();
    let (fixed, resolver) = setup(None);
    let ctx = ResolveContext {
        import_map: HashMap::from([("lib".to_owned(), "./a.js".to_owned())]),
        import_map_dir: root.clone(),
        ..Default::default()
    };
    resolver.resolve("./a.js", &referrer(&root), &ctx).unwrap();
    resolver.resolve("lib", &referrer(&root), &ctx).unwrap();
    assert!(fixed.calls.lock().unwrap().is_empty());
}

#[test]
fn strategy_error_propagates_unchanged() {
    let (_d, root) = project();
    let (_fixed, resolver) = setup(None);
    let err = resolver
        .resolve("pkg", &referrer(&root), &ResolveContext::default())
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
    assert_eq!(err.message(), "strategy says no");
}

#[test]
fn strategy_target_that_does_not_exist_is_not_found() {
    let (_d, root) = project();
    let (_fixed, resolver) = setup(Some(root.join("missing.js")));
    let err = resolver
        .resolve("pkg", &referrer(&root), &ResolveContext::default())
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[test]
fn import_local_on_root_covers_node_modules_and_nothing_else_does() {
    let (_d, root) = project();
    let (_fixed, resolver) = setup(Some(root.join("node_modules/pkg/index.js")));
    let module = resolver
        .resolve("pkg", &referrer(&root), &ResolveContext::default())
        .unwrap();
    let host = ModuleHost::new(Arc::new(FsLoader));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let grant = |pattern: &Path| LoadContext {
        permissions: CapabilitySet::builder()
            .allow(Capability::ImportLocal(PathPattern::new(
                pattern.to_str().unwrap(),
            )))
            .build(),
    };
    assert!(runtime.block_on(host.load(&module, &grant(&root))).is_ok());
    let other = root.join("elsewhere");
    let err = runtime
        .block_on(host.load(&module, &grant(&other)))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PermissionDenied);
}

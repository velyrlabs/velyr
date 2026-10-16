//! The `SourceTransform` hook on `ModuleHost`.

use std::fs;
use std::sync::{Arc, Mutex};

use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_modules::{
    FsLoader, FsResolver, LoadContext, MediaType, ModuleHost, ModuleId, ModuleResolver,
    ResolveContext, Result, SourceTransform, Transformed,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

/// Upper-cases the text, records what it was called with, and fails on `FAIL`.
#[derive(Default)]
struct Shout(Mutex<Vec<(String, MediaType)>>);

impl SourceTransform for Shout {
    fn transform(
        &self,
        specifier: &str,
        source: &str,
        media_type: MediaType,
    ) -> Result<Transformed> {
        self.0
            .lock()
            .unwrap()
            .push((specifier.to_owned(), media_type));
        if source.contains("FAIL") {
            return Err(VelyrError::invalid_data(format!("{specifier}:1:1: boom")));
        }
        Ok(Transformed {
            code: source.to_uppercase(),
            source_map: Some("{\"version\":3}".to_owned()),
        })
    }
}

fn setup(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf, LoadContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for (name, text) in files {
        fs::write(root.join(name), text).unwrap();
    }
    let permissions = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            root.to_str().unwrap(),
        )))
        .build();
    (dir, root, LoadContext { permissions })
}

fn resolve(root: &std::path::Path, name: &str) -> velyr_runtime_modules::ResolvedModule {
    let referrer = ModuleId::from_path(&root.join("main.ts")).unwrap();
    FsResolver
        .resolve(&format!("./{name}"), &referrer, &ResolveContext::default())
        .unwrap()
}

#[tokio::test]
async fn host_loads_the_transformed_text_with_its_source_map() {
    let (_d, root, ctx) = setup(&[("a.ts", "let a: number = 1"), ("b.js", "let b = 2")]);
    let shout = Arc::new(Shout::default());
    let host = ModuleHost::new(Arc::new(FsLoader)).with_transform(shout.clone());
    let module = resolve(&root, "a.ts");
    let source = host.load(&module, &ctx).await.unwrap();
    assert_eq!(source.text, "LET A: NUMBER = 1");
    assert_eq!(
        source.hash,
        velyr_runtime_modules::ModuleSource::new(source.text.clone()).hash
    );
    assert_eq!(
        host.source_map(&module.id).as_deref(),
        Some("{\"version\":3}")
    );
    host.load(&resolve(&root, "b.js"), &ctx).await.unwrap();
    let calls = shout.0.lock().unwrap().clone();
    assert_eq!(
        calls[0],
        (
            module.path.to_string_lossy().into_owned(),
            MediaType::TypeScript
        )
    );
    assert_eq!(calls[1].1, MediaType::JavaScript);
}

#[tokio::test]
async fn transformed_modules_are_cached_by_id() {
    let (_d, root, ctx) = setup(&[("a.ts", "x")]);
    let shout = Arc::new(Shout::default());
    let host = ModuleHost::new(Arc::new(FsLoader)).with_transform(shout.clone());
    let module = resolve(&root, "a.ts");
    host.load(&module, &ctx).await.unwrap();
    host.load(&module, &ctx).await.unwrap();
    assert_eq!(shout.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn transform_errors_reach_the_caller_and_nothing_is_cached() {
    let (_d, root, ctx) = setup(&[("a.ts", "FAIL")]);
    let host = ModuleHost::new(Arc::new(FsLoader)).with_transform(Arc::new(Shout::default()));
    let module = resolve(&root, "a.ts");
    let err = host.load(&module, &ctx).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
    assert!(err.message().contains("a.ts:1:1"));
    assert!(host.source_map(&module.id).is_none());
}

#[tokio::test]
async fn without_a_transform_text_is_untouched() {
    let (_d, root, ctx) = setup(&[("a.ts", "let a = 1")]);
    let host = ModuleHost::new(Arc::new(FsLoader));
    let source = host.load(&resolve(&root, "a.ts"), &ctx).await.unwrap();
    assert_eq!(source.text, "let a = 1");
}

#[test]
fn media_types_follow_the_extension() {
    use std::path::Path;
    for (file, expected) in [
        ("a.ts", MediaType::TypeScript),
        ("a.mts", MediaType::Mts),
        ("a.cts", MediaType::Cts),
        ("a.tsx", MediaType::Tsx),
        ("a.js", MediaType::JavaScript),
        ("a", MediaType::JavaScript),
    ] {
        assert_eq!(MediaType::from_path(Path::new(file)), expected, "{file}");
    }
}

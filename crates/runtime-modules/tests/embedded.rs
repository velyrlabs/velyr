use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use velyr_runtime_errors::ErrorCode;
use velyr_runtime_modules::{
    FsLoader, FsResolver, LoadContext, ModuleHost, ModuleId, ModuleResolver, ResolveContext,
    is_embedded,
};
use velyr_runtime_permissions::CapabilitySet;

const EXPECTED_VERSION: &str = "0.154.0";
const HELPER_SHA256: [(&str, &str); 4] = [
    (
        "decorate",
        "e004200fc4d7b2b934163a195018714b6c39aac7acb5db55ec3c8fb973a7fc95",
    ),
    (
        "decorateParam",
        "9ad4ad0c08209b4297f52ef00b36486743a34423d13a22a7d5cb07ed8810f54d",
    ),
    (
        "decorateMetadata",
        "767708ad199c0c993522d5a0ca2487b1bc29e201658a7402de7f5821ec435829",
    ),
    (
        "usingCtx",
        "638141c6ff255fd00c4a1f7c1e0d2b15431d60954c3f963e152d604cfe27bc26",
    ),
];

fn referrer() -> ModuleId {
    ModuleId::from_path(Path::new("/proj/main.ts")).unwrap()
}

fn no_grants() -> LoadContext {
    LoadContext {
        permissions: Arc::new(CapabilitySet::default()),
    }
}

#[test]
fn helper_specifier_resolves_to_reserved_path() {
    let resolved = FsResolver
        .resolve(
            "velyr-oxc-runtime/helpers/decorate",
            &referrer(),
            &ResolveContext::default(),
        )
        .unwrap();
    assert_eq!(
        resolved.path,
        PathBuf::from("/$velyr/oxc-runtime/helpers/decorate.js")
    );
}

#[tokio::test]
async fn helper_loads_without_permissions_or_disk() {
    let resolved = FsResolver
        .resolve(
            "velyr-oxc-runtime/helpers/usingCtx",
            &referrer(),
            &ResolveContext::default(),
        )
        .unwrap();
    let host = ModuleHost::new(Arc::new(FsLoader));
    let source = host.load(&resolved, &no_grants()).await.unwrap();
    assert!(source.text.contains("export { _usingCtx as default }"));
}

#[test]
fn unknown_helper_is_not_found() {
    let err = FsResolver
        .resolve(
            "velyr-oxc-runtime/helpers/nope",
            &referrer(),
            &ResolveContext::default(),
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[test]
fn import_map_cannot_shadow_helpers() {
    let ctx = ResolveContext {
        import_map: HashMap::from([("velyr-oxc-runtime/".into(), "./evil/".into())]),
        import_map_dir: PathBuf::from("/proj"),
        ..Default::default()
    };
    let other = ModuleId::from_path(Path::new("/elsewhere/deep/x.js")).unwrap();
    let resolved = FsResolver
        .resolve("velyr-oxc-runtime/helpers/decorateParam", &other, &ctx)
        .unwrap();
    assert_eq!(
        resolved.path,
        PathBuf::from("/$velyr/oxc-runtime/helpers/decorateParam.js")
    );
}

#[test]
fn vendored_helpers_are_pinned() {
    assert_eq!(velyr_runtime_modules::OXC_HELPERS_VERSION, EXPECTED_VERSION);
    for (name, expected) in HELPER_SHA256 {
        let bytes =
            std::fs::read(format!("{}/helpers/{name}.js", env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), expected, "{name}");
    }
}

#[test]
fn is_embedded_matches_only_helper_paths() {
    assert!(is_embedded(Path::new(
        "/$velyr/oxc-runtime/helpers/decorate.js"
    )));
    assert!(!is_embedded(Path::new(
        "/$velyr/oxc-runtime/helpers/nope.js"
    )));
    assert!(!is_embedded(Path::new(
        "/tmp/oxc-runtime/helpers/decorate.js"
    )));
    assert!(!is_embedded(Path::new(
        "/$velyr/oxc-runtime/helpers/decorate.ts"
    )));
}

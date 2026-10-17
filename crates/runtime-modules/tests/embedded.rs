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
const HELPER_SHA256: [(&str, &str); 10] = [
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
    (
        "applyDecs2311",
        "eef15d7649820ad235b357a2d54e99dab5eb61db5883c10ab4dc5bda31b624a9",
    ),
    (
        "typeof",
        "928bcbbdf74fe24ed7486e7f982c4389c1859886b3ed904ebd2677f22193b57a",
    ),
    (
        "checkInRHS",
        "c02f507c1016ee027e2a25ec6ee927125c44d8906304d7d4d45c948b03c30da6",
    ),
    (
        "setFunctionName",
        "45515de6a2a12402f9288a7902f3c5d651cad437c212e61071514a1dd13ab042",
    ),
    (
        "toPropertyKey",
        "7f0000acc9c9cf7594b412095be66cdc59066b25458a16fe1434fd00cff478e7",
    ),
    (
        "toPrimitive",
        "9660291e71923cb5ff9aaaa1de354515c26e3220cba56ac5d21b806f23875545",
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

fn helper_referrer() -> ModuleId {
    ModuleId::from_path(Path::new("/$velyr/oxc-runtime/helpers/applyDecs2311.js")).unwrap()
}

#[test]
fn relative_import_inside_a_helper_resolves_inside_the_table() {
    let resolved = FsResolver
        .resolve("./typeof.js", &helper_referrer(), &ResolveContext::default())
        .unwrap();
    assert_eq!(
        resolved.path,
        PathBuf::from("/$velyr/oxc-runtime/helpers/typeof.js")
    );
}

#[test]
fn relative_import_inside_a_helper_cannot_leave_the_table() {
    for specifier in ["./nope.js", "../helpers/typeof.js", "./sub/typeof.js"] {
        let err = FsResolver
            .resolve(specifier, &helper_referrer(), &ResolveContext::default())
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound, "{specifier}");
    }
}

#[test]
fn relative_import_from_a_disk_module_never_reaches_the_table() {
    let err = FsResolver
        .resolve("./typeof.js", &referrer(), &ResolveContext::default())
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
}

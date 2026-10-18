//! The facade table is a process-wide `OnceLock`, so everything that registers it lives in this
//! one test binary and in a single ordered test.

use std::path::Path;
use std::sync::Arc;

use velyr_runtime_errors::ErrorCode;
use velyr_runtime_modules::{
    FsLoader, LoadContext, ModuleHost, ModuleId, ResolvedModule, is_embedded,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

const NAMES: &[&str] = &["path", "fs"];

fn facade_source(name: &str) -> Option<String> {
    NAMES
        .contains(&name)
        .then(|| format!("export default {name:?};\n"))
}

fn resolved(path: &Path) -> ResolvedModule {
    ResolvedModule {
        id: ModuleId::from_path(path).unwrap(),
        path: path.to_path_buf(),
    }
}

#[tokio::test]
async fn is_embedded_covers_exactly_the_registered_facade_names() {
    let helper = Path::new("/$velyr/oxc-runtime/helpers/decorate.js");
    let lookalike_dir = tempfile::tempdir().unwrap();
    let lookalike = lookalike_dir.path().join("$velyr/node/path.js");

    assert!(!is_embedded(Path::new("/$velyr/node/path.js")));
    ModuleHost::register_embedded_facades(NAMES, facade_source);

    assert!(is_embedded(Path::new("/$velyr/node/path.js")));
    assert!(is_embedded(Path::new("/$velyr/node/fs.js")));
    assert!(is_embedded(helper));
    for rejected in [
        "/$velyr/node/nope.js",
        "/$velyr/node/path.ts",
        "/$velyr/node/path",
        "/$velyr/node/path.js/",
        "/$velyr/node/../oxc-runtime/helpers/x.js",
        "/$velyr/node/../node/path.js",
        "/$velyr/node/./path.js",
        "/$velyr/nodes/path.js",
        "/$velyr/node/path.js.map",
    ] {
        assert!(!is_embedded(Path::new(rejected)), "{rejected}");
    }
    assert!(!is_embedded(&lookalike));

    let no_grants = LoadContext {
        permissions: Arc::new(CapabilitySet::default()),
    };
    let host = ModuleHost::new(Arc::new(FsLoader));
    let facade = resolved(Path::new("/$velyr/node/path.js"));
    let source = host.load(&facade, &no_grants).await.unwrap();
    assert_eq!(&*source.text, "export default \"path\";\n");
}

#[tokio::test]
async fn unregistered_facade_name_is_loaded_from_disk_with_authorization() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let file = root.join("$velyr/node/path.js");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "export default 'from disk';\n").unwrap();
    let module = resolved(&file);
    let host = ModuleHost::new(Arc::new(FsLoader));

    let denied = LoadContext {
        permissions: Arc::new(CapabilitySet::default()),
    };
    let err = host.load(&module, &denied).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::PermissionDenied);

    let granted = LoadContext {
        permissions: CapabilitySet::builder()
            .allow(Capability::ImportLocal(PathPattern::new(
                root.to_str().unwrap(),
            )))
            .build(),
    };
    let source = host.load(&module, &granted).await.unwrap();
    assert!(source.text.contains("from disk"));
}

//! TC39 2023-11 decorators and `accessor` go through the lowering pass; legacy mode wins.

use std::sync::Arc;

use velyr_runtime_modules::{MediaType, SourceTransform};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_tooling_tsconfig::TsconfigLoader;
use velyr_tooling_typescript::TypeScriptTransform;

const SOURCE: &str =
    "function dec(v: any, c: any) { return v; }\nexport class A { @dec m() {} accessor x = 1; }";

fn run(tsconfig: Option<&str>, name: &str, media_type: MediaType) -> String {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let mut transform = TypeScriptTransform::new(None);
    if let Some(text) = tsconfig {
        std::fs::write(root.join("tsconfig.json"), text).unwrap();
        let permissions = CapabilitySet::builder()
            .allow(Capability::ImportLocal(PathPattern::new(
                root.to_str().unwrap(),
            )))
            .build();
        transform = transform.with_tsconfig(Arc::new(TsconfigLoader::new(permissions)));
    }
    let path = root.join(name);
    transform
        .transform(path.to_str().unwrap(), SOURCE, media_type)
        .unwrap()
        .code
}

#[test]
fn lowers_decorators_and_accessor_through_the_pass() {
    let code = run(None, "a.ts", MediaType::TypeScript);
    assert!(
        code.contains("velyr-oxc-runtime/helpers/applyDecs2311"),
        "{code}"
    );
    assert!(!code.contains("@dec"), "{code}");
    assert!(!code.contains("accessor x"), "{code}");
}

#[test]
fn experimental_decorators_keep_the_legacy_path() {
    let code = run(
        Some(r#"{"compilerOptions":{"experimentalDecorators":true}}"#),
        "a.ts",
        MediaType::TypeScript,
    );
    assert!(
        code.contains("velyr-oxc-runtime/helpers/decorate"),
        "{code}"
    );
    assert!(!code.contains("applyDecs2311"), "{code}");
}

#[test]
fn mts_and_tsx_take_the_same_path() {
    for (name, media_type) in [("a.mts", MediaType::Mts), ("a.tsx", MediaType::Tsx)] {
        let code = run(None, name, media_type);
        assert!(code.contains("applyDecs2311"), "{name}: {code}");
    }
}

#[test]
fn plain_javascript_is_not_handled() {
    assert!(!TypeScriptTransform::new(None).handles(MediaType::JavaScript));
}

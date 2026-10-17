//! `JsxTransform` derives its options per file from the nearest tsconfig (T11).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use velyr_runtime_modules::{MediaType, SourceTransform};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_tooling_jsx::{JsxOverrides, JsxTransform};
use velyr_tooling_tsconfig::TsconfigLoader;

const PROJECT_A: &str = "{ // jsx\n \"compilerOptions\": { \"jsx\": \"react\", \"jsxFactory\": \"h\", \"experimentalDecorators\": true, } }";
const PROJECT_B: &str =
    "{ \"compilerOptions\": { \"jsx\": \"react-jsx\", \"jsxImportSource\": \"preact\" } }";
const ELEMENT: &str = "export const v = <i/>;\n";

fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

fn workspace() -> (tempfile::TempDir, PathBuf, JsxTransform) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    write(&root, "a/tsconfig.json", PROJECT_A);
    write(&root, "b/tsconfig.json", PROJECT_B);
    let permissions = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            root.to_str().unwrap(),
        )))
        .build();
    let transform =
        JsxTransform::default().with_tsconfig(Arc::new(TsconfigLoader::new(permissions)));
    (dir, root, transform)
}

fn run(transform: &JsxTransform, file: &Path, source: &str) -> String {
    transform
        .transform(file.to_str().unwrap(), source, MediaType::Tsx)
        .unwrap()
        .code
}

#[test]
fn each_file_uses_its_own_tsconfig() {
    let (_dir, root, transform) = workspace();
    let a = run(&transform, &root.join("a/x.tsx"), ELEMENT);
    assert!(a.contains("h(\"i\", null)"), "{a}");
    let b = run(&transform, &root.join("b/y.tsx"), ELEMENT);
    assert!(b.contains("preact/jsx-runtime"), "{b}");
}

#[test]
fn legacy_decorators_apply_to_tsx() {
    let (_dir, root, transform) = workspace();
    let source = "function D(): any { return () => {}; }\nexport class A { @D() m() {} }\n";
    let code = run(&transform, &root.join("a/x.tsx"), source);
    assert!(
        code.contains("velyr-oxc-runtime/helpers/decorate"),
        "{code}"
    );
    assert!(!code.contains("@D"), "{code}");
}

#[test]
fn overrides_beat_tsconfig() {
    let (_dir, root, transform) = workspace();
    let transform = transform.with_overrides(JsxOverrides {
        jsx: Some("react-jsx".into()),
        import_source: Some("solid".into()),
        ..JsxOverrides::default()
    });
    let code = run(&transform, &root.join("a/x.tsx"), ELEMENT);
    assert!(code.contains("solid/jsx-runtime"), "{code}");
}

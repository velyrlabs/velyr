use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_tooling_tsconfig::{CompilerOptions, TsconfigLoader};

const BASE: &str = r#"{ // base
  "compilerOptions": { "experimentalDecorators": true, "target": "ES2021", "baseUrl": ".",
    "paths": { "@/*": ["src/*"], }, } }"#;
const APP: &str = r#"{ "extends": "../tsconfig.base.json", "compilerOptions": {
  "emitDecoratorMetadata": true, "paths": { "@lib/*": ["../lib/*", "../lib2/*"] } } }"#;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn allow(dir: &Path) -> Arc<CapabilitySet> {
    CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            dir.to_str().unwrap(),
        )))
        .build()
}

/// Fixture with a base config and an app config extending it, plus a loader allowed on the tree.
fn project() -> (TempDir, PathBuf, TsconfigLoader) {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    write(&root, "tsconfig.base.json", BASE);
    write(&root, "app/tsconfig.json", APP);
    let loader = TsconfigLoader::new(allow(&root));
    (dir, root, loader)
}

#[test]
fn jsonc_comments_and_trailing_commas_parse() {
    let (_dir, root, loader) = project();
    let config = loader.for_file(&root.join("tsconfig.base.json")).unwrap();
    assert!(
        config.is_none(),
        "no tsconfig.json at the root, only a base file"
    );
    let parsed = loader
        .for_file(&root.join("app/src/x.ts"))
        .unwrap()
        .unwrap();
    assert_eq!(parsed.options.target.as_deref(), Some("ES2021"));
}

#[test]
fn nearest_config_is_found_from_the_file() {
    let (_dir, root, loader) = project();
    let config = loader
        .for_file(&root.join("app/src/x.ts"))
        .unwrap()
        .unwrap();
    assert_eq!(config.path, root.join("app/tsconfig.json"));
}

#[test]
fn extends_merges_options_and_replaces_paths() {
    let (_dir, root, loader) = project();
    let options = loader
        .for_file(&root.join("app/src/x.ts"))
        .unwrap()
        .unwrap()
        .options
        .clone();
    assert_eq!(options.experimental_decorators, Some(true));
    assert_eq!(options.emit_decorator_metadata, Some(true));
    assert_eq!(options.base_url.as_deref(), Some(root.as_path()));
    let paths = options.paths.unwrap();
    assert_eq!(paths.map.keys().collect::<Vec<_>>(), ["@lib/*"]);
    assert_eq!(paths.base, root);
}

#[test]
fn paths_base_is_the_defining_directory_without_base_url() {
    let (_dir, root, loader) = project();
    write(
        &root,
        "plain/tsconfig.json",
        r#"{"compilerOptions":{"paths":{"a":["b"]}}}"#,
    );
    let options = loader
        .for_file(&root.join("plain/x.ts"))
        .unwrap()
        .unwrap()
        .options
        .clone();
    assert_eq!(options.paths.unwrap().base, root.join("plain"));
}

#[test]
fn extends_array_later_entry_wins() {
    let (_dir, root, loader) = project();
    write(
        &root,
        "a.json",
        r#"{"compilerOptions":{"target":"ES5","jsx":"react"}}"#,
    );
    write(
        &root,
        "b.json",
        r#"{"compilerOptions":{"target":"ES2022"}}"#,
    );
    write(
        &root,
        "m/tsconfig.json",
        r#"{"extends":["../a.json","../b"]}"#,
    );
    let options = loader
        .for_file(&root.join("m/x.ts"))
        .unwrap()
        .unwrap()
        .options
        .clone();
    assert_eq!(options.target.as_deref(), Some("ES2022"));
    assert_eq!(options.jsx.as_deref(), Some("react"));
}

#[test]
fn extends_cycle_is_an_error() {
    let (_dir, root, loader) = project();
    write(&root, "cyc/tsconfig.json", r#"{"extends":"./b.json"}"#);
    write(&root, "cyc/b.json", r#"{"extends":"./tsconfig.json"}"#);
    assert!(loader.for_file(&root.join("cyc/x.ts")).is_err());
}

#[test]
fn extends_depth_above_sixteen_is_an_error() {
    let (_dir, root, loader) = project();
    write(&root, "deep/tsconfig.json", r#"{"extends":"./c0.json"}"#);
    for i in 0..20 {
        write(
            &root,
            &format!("deep/c{i}.json"),
            &format!(r#"{{"extends":"./c{}.json"}}"#, i + 1),
        );
    }
    write(&root, "deep/c20.json", "{}");
    assert!(loader.for_file(&root.join("deep/x.ts")).is_err());
}

#[test]
fn package_extends_resolves_through_node_modules() {
    let (_dir, root, loader) = project();
    write(
        &root,
        "node_modules/@tsconfig/node20/tsconfig.json",
        r#"{"compilerOptions":{"target":"ES2022"}}"#,
    );
    write(
        &root,
        "pkg/sub/tsconfig.json",
        r#"{"extends":"@tsconfig/node20/tsconfig.json"}"#,
    );
    let options = loader
        .for_file(&root.join("pkg/sub/x.ts"))
        .unwrap()
        .unwrap()
        .options
        .clone();
    assert_eq!(options.target.as_deref(), Some("ES2022"));
}

#[test]
fn files_under_node_modules_get_no_config() {
    let (_dir, root, loader) = project();
    assert!(
        loader
            .for_file(&root.join("app/node_modules/pkg/x.ts"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn unauthorized_root_returns_none_without_reading() {
    let (_dir, root, _) = project();
    let other = tempfile::tempdir().unwrap();
    let loader = TsconfigLoader::new(allow(&dunce::canonicalize(other.path()).unwrap()));
    assert!(
        loader
            .for_file(&root.join("app/src/x.ts"))
            .unwrap()
            .is_none()
    );
    assert_eq!(loader.reads(), 0);
}

#[test]
fn broken_json_names_the_file() {
    let (_dir, root, loader) = project();
    write(&root, "bad/tsconfig.json", "{ not json");
    let err = loader.for_file(&root.join("bad/x.ts")).unwrap_err();
    assert!(
        err.message()
            .replace('\\', "/")
            .contains("bad/tsconfig.json"),
        "{}",
        err.message()
    );
}

#[test]
fn second_call_hits_the_cache() {
    let (_dir, root, loader) = project();
    let file = root.join("app/src/x.ts");
    let first = loader.for_file(&file).unwrap().unwrap();
    let reads = loader.reads();
    assert_eq!(reads, 2, "app config plus its base");
    let second = loader.for_file(&file).unwrap().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(loader.reads(), reads);
}

fn with_target(target: Option<&str>, flag: Option<bool>) -> CompilerOptions {
    CompilerOptions {
        target: target.map(str::to_owned),
        use_define_for_class_fields: flag,
        ..Default::default()
    }
}

#[test]
fn define_class_fields_follows_flag_then_target() {
    assert!(with_target(None, None).define_class_fields());
    assert!(!with_target(Some("ES2021"), None).define_class_fields());
    assert!(with_target(Some("ES2022"), None).define_class_fields());
    assert!(with_target(Some("ESNext"), None).define_class_fields());
    assert!(!with_target(Some("ES5"), None).define_class_fields());
    assert!(with_target(Some("ES2021"), Some(true)).define_class_fields());
    assert!(!with_target(Some("ESNext"), Some(false)).define_class_fields());
}

#[test]
fn strict_null_checks_falls_back_to_strict() {
    let mut options = CompilerOptions::default();
    assert!(!options.strict_null_checks());
    options.strict = Some(true);
    assert!(options.strict_null_checks());
    options.strict_null_checks = Some(false);
    assert!(!options.strict_null_checks());
}

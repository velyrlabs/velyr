//! `extends` resolution stays inside the grant and is bounded in files read and bytes per file.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_tooling_tsconfig::TsconfigLoader;

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

#[cfg(unix)]
#[test]
fn package_extends_cannot_climb_out_of_the_grant_through_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let grant = root.join("grant");
    write(
        &root,
        "grant/proj/tsconfig.json",
        r#"{"extends":"a/../../../s.json"}"#,
    );
    write(&root, "s.json", r#"{"compilerOptions":{"target":"ES5"}}"#);
    fs::create_dir_all(root.join("far/q/r")).unwrap();
    fs::create_dir_all(grant.join("proj/node_modules")).unwrap();
    std::os::unix::fs::symlink("../../../far/q/r", grant.join("proj/node_modules/a")).unwrap();

    let loader = TsconfigLoader::new(allow(&grant));
    let result = loader.for_file(&grant.join("proj/x.ts"));
    assert!(result.is_err(), "outside file must not be read: {result:?}");
}

#[test]
fn extends_diamond_parses_each_file_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let levels = 10;
    write(&root, "tsconfig.json", r#"{"extends":"./c0.json"}"#);
    for i in 0..levels {
        let next = format!("./c{}.json", i + 1);
        let parents = format!(r#"{{"extends":["{next}","{next}","{next}","{next}"]}}"#);
        write(&root, &format!("c{i}.json"), &parents);
    }
    write(
        &root,
        &format!("c{levels}.json"),
        r#"{"compilerOptions":{"target":"ES2021"}}"#,
    );

    let loader = TsconfigLoader::new(allow(&root));
    let config = loader.for_file(&root.join("x.ts")).unwrap().unwrap();
    assert_eq!(config.options.target.as_deref(), Some("ES2021"));
    assert_eq!(loader.reads(), levels + 2);
}

#[test]
fn too_many_extends_files_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let parents: Vec<String> = (0..70).map(|i| format!("\"./p{i}.json\"")).collect();
    write(
        &root,
        "tsconfig.json",
        &format!(r#"{{"extends":[{}]}}"#, parents.join(",")),
    );
    for i in 0..70 {
        write(&root, &format!("p{i}.json"), "{}");
    }
    let loader = TsconfigLoader::new(allow(&root));
    let err = loader.for_file(&root.join("x.ts")).unwrap_err();
    assert!(err.message().contains("more than 64"), "{}", err.message());
}

#[test]
fn oversized_config_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let padding = " ".repeat(2 * 1024 * 1024);
    write(&root, "tsconfig.json", &format!("{{}}{padding}"));
    let loader = TsconfigLoader::new(allow(&root));
    let err = loader.for_file(&root.join("x.ts")).unwrap_err();
    assert!(
        err.message().contains("larger than 1 MiB"),
        "{}",
        err.message()
    );
}

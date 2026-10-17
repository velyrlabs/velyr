//! `node:x` facades are served from memory: nothing is written at start and the embedded path
//! needs no `ImportLocal` grant, while real files still do.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use velyr_compat_node::{NodeBuiltinResolver, install_compat_node};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_modules::{FsResolver, ModuleId, ModuleResolver, ResolveContext, is_embedded};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

fn canonical(dir: &tempfile::TempDir) -> PathBuf {
    std::fs::canonicalize(dir.path()).unwrap()
}

fn run(work: &Path, facades: &Path, grants: &[&Path]) -> Result<(), String> {
    let mut capabilities = CapabilitySet::builder();
    for grant in grants {
        capabilities = capabilities.allow(Capability::ImportLocal(PathPattern::new(
            grant.to_str().unwrap(),
        )));
    }
    let resolver = NodeBuiltinResolver::new(Arc::new(FsResolver), facades).unwrap();
    let config = RuntimeConfig {
        capabilities: Some(capabilities.build()),
        platform_init: Some(V8Engine::init_platform),
        module_resolver: Some(Arc::new(resolver)),
        ..RuntimeConfig::default()
    };
    run_module_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        |isolate, _, _| install_compat_node(isolate).expect("install"),
        work.join("main.mjs"),
    )
    .map_err(|e| format!("{e:?}"))
}

#[test]
fn starting_does_not_write_facade_files() {
    let (work, facades) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (work, facades) = (canonical(&work), canonical(&facades));
    std::fs::write(
        work.join("main.mjs"),
        "import { join } from 'node:path'; join('a');",
    )
    .unwrap();
    run(&work, &facades, &[&work]).unwrap();
    assert_eq!(std::fs::read_dir(&facades).unwrap().count(), 0);
}

#[test]
fn node_builtin_import_resolves_to_the_embedded_path() {
    let facades = tempfile::tempdir().unwrap();
    let resolver = NodeBuiltinResolver::new(Arc::new(FsResolver), &canonical(&facades)).unwrap();
    let referrer = ModuleId::from_path(Path::new("/proj/main.mjs")).unwrap();
    for (specifier, path) in [
        ("node:path", "/$velyr/node/path.js"),
        ("path", "/$velyr/node/path.js"),
        ("node:fs/promises", "/$velyr/node/fs/promises.js"),
    ] {
        let found = resolver
            .resolve(specifier, &referrer, &ResolveContext::default())
            .unwrap();
        assert_eq!(found.path, Path::new(path), "{specifier}");
        assert!(is_embedded(&found.path), "{specifier}");
    }
}

#[test]
fn embedded_path_needs_no_import_grant() {
    let (work, facades, outside) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (work, facades, outside) = (canonical(&work), canonical(&facades), canonical(&outside));
    std::fs::write(outside.join("dep.mjs"), "export const v = 1;").unwrap();
    std::fs::write(
        work.join("main.mjs"),
        "import 'node:path'; import 'node:fs';",
    )
    .unwrap();
    run(&work, &facades, &[&work]).unwrap();

    let dep = outside.join("dep.mjs");
    std::fs::write(
        work.join("main.mjs"),
        format!("import 'node:path'; import {:?};", dep.to_str().unwrap()),
    )
    .unwrap();
    assert!(
        run(&work, &facades, &[&work]).is_err(),
        "file outside the grant must still fail"
    );
}

#[test]
fn node_builtin_import_works_for_esm() {
    let (work, facades) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (work, facades) = (canonical(&work), canonical(&facades));
    std::fs::write(
        work.join("main.mjs"),
        "import path, { sep } from 'node:path';\n\
         import { readFile } from 'node:fs/promises';\n\
         if (path.sep !== sep || typeof readFile !== 'function') throw new Error('bad');",
    )
    .unwrap();
    run(&work, &facades, &[&work]).unwrap();
}

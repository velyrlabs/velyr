#![no_main]

//! Arbitrary specifiers against a fixed project: `FsResolver` must never panic, and a module
//! only loads (`ModuleHost` with an `ImportLocal` grant on `<root>/proj`) when its canonical
//! path is under the grant, even through `..`, absolute paths, file URLs, import-map entries
//! and a symlink that points out of the project.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use libfuzzer_sys::fuzz_target;
use velyr_runtime_modules::{
    FsLoader, FsResolver, LoadContext, ModuleHost, ModuleId, ModuleResolver, ResolveContext,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

struct Fixture {
    _dir: tempfile::TempDir,
    proj: PathBuf,
    referrer: ModuleId,
    resolve_ctx: ResolveContext,
    load_ctx: LoadContext,
    runtime: tokio::runtime::Runtime,
}

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonical root");
        let proj = root.join("proj");
        let outside = root.join("outside");
        fs::create_dir_all(proj.join("sub")).expect("fixture dirs");
        fs::create_dir_all(&outside).expect("fixture dirs");
        for file in [
            proj.join("main.js"),
            proj.join("a.js"),
            proj.join("sub/b.js"),
            outside.join("secret.js"),
        ] {
            fs::write(file, "export {};").expect("fixture file");
        }
        std::os::unix::fs::symlink(&outside, proj.join("escape")).expect("symlink");
        let import_map = HashMap::from([
            ("lib".to_owned(), "./a.js".to_owned()),
            ("up/".to_owned(), "../outside/".to_owned()),
            (
                "abs".to_owned(),
                outside.join("secret.js").to_string_lossy().into_owned(),
            ),
        ]);
        let permissions = CapabilitySet::builder()
            .allow(Capability::ImportLocal(PathPattern::new(
                proj.to_str().expect("utf-8 temp path"),
            )))
            .build();
        Fixture {
            referrer: ModuleId::from_path(&proj.join("sub/b.js")).expect("referrer id"),
            resolve_ctx: ResolveContext {
                import_map,
                import_map_dir: proj.clone(),
            },
            load_ctx: LoadContext { permissions },
            runtime: tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("runtime"),
            proj,
            _dir: dir,
        }
    })
}

fuzz_target!(|specifier: &str| {
    let fx = fixture();
    let Ok(resolved) = FsResolver.resolve(specifier, &fx.referrer, &fx.resolve_ctx) else {
        return;
    };
    assert!(
        resolved.path.is_absolute(),
        "non-absolute path for {specifier:?}"
    );
    assert_eq!(
        resolved.id.to_path().as_deref(),
        Some(resolved.path.as_path()),
        "id does not round-trip for {specifier:?}"
    );
    let host = ModuleHost::new(Arc::new(FsLoader));
    let loaded = fx.runtime.block_on(host.load(&resolved, &fx.load_ctx));
    if loaded.is_ok() {
        assert!(
            resolved.path.starts_with(&fx.proj),
            "loaded outside the grant: {specifier:?} -> {:?}",
            resolved.path
        );
    }
});

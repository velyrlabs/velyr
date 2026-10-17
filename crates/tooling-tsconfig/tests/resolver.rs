use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tempfile::TempDir;
use velyr_runtime_errors::ErrorCode;
use velyr_runtime_modules::{
    FsResolver, ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_tooling_tsconfig::{ImportMap, ProjectResolver, TsconfigLoader};

const TSCONFIG: &str = r#"{ "compilerOptions": { "baseUrl": ".", "paths": {
  "@/*": ["src/*"], "@lib/*": ["lib/*", "vendor/*"], "exact": ["src/exact.ts"],
  "escape/*": ["../../outside/*"], "velyr-oxc-runtime/*": ["evil/*"] } } }"#;

/// Wraps `FsResolver`, recording every specifier and context the wrapper hands down.
#[derive(Default)]
struct Recording {
    calls: Mutex<Vec<(String, ResolveContext)>>,
}

struct Spy(Arc<Recording>);

impl ModuleResolver for Spy {
    fn resolve(
        &self,
        spec: &str,
        referrer: &ModuleId,
        ctx: &ResolveContext,
    ) -> Result<ResolvedModule> {
        self.0
            .calls
            .lock()
            .unwrap()
            .push((spec.to_owned(), ctx.clone()));
        FsResolver.resolve(spec, referrer, ctx)
    }
}

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    recording: Arc<Recording>,
}

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

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    write(&root, "proj/tsconfig.json", TSCONFIG);
    for f in [
        "main.ts",
        "src/util.ts",
        "src/exact.ts",
        "vendor/only.ts",
        "keep.ts",
        "evil/helpers/decorate.ts",
    ] {
        write(&root, &format!("proj/{f}"), "");
    }
    write(&root, "outside/secret.ts", "");
    write(&root, "proj/node_modules/pkg/x.ts", "");
    Fixture {
        _dir: dir,
        root,
        recording: Arc::default(),
    }
}

impl Fixture {
    fn resolver(&self, imports: Option<ImportMap>) -> ProjectResolver {
        let proj = self.root.join("proj");
        let permissions = allow(&proj);
        let loader = Arc::new(TsconfigLoader::new(permissions.clone()));
        ProjectResolver::new(
            Arc::new(Spy(self.recording.clone())),
            loader,
            permissions,
            imports,
        )
    }

    fn referrer(&self, rel: &str) -> ModuleId {
        ModuleId::from_path(&self.root.join("proj").join(rel)).unwrap()
    }

    fn resolve(&self, resolver: &ProjectResolver, spec: &str) -> Result<ResolvedModule> {
        resolver.resolve(spec, &self.referrer("main.ts"), &ResolveContext::default())
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join("proj").join(rel)
    }
}

fn tilde_map(f: &Fixture) -> ImportMap {
    ImportMap {
        dir: f.path(""),
        entries: HashMap::from([("~/".to_owned(), "./src/".to_owned())]),
    }
}

#[test]
fn paths_pattern_resolves_to_the_target() {
    let f = fixture();
    assert_eq!(
        f.resolve(&f.resolver(None), "@/util").unwrap().path,
        f.path("src/util.ts")
    );
}

#[test]
fn exact_key_beats_pattern_and_second_target_is_tried() {
    let f = fixture();
    let r = f.resolver(None);
    assert_eq!(f.resolve(&r, "exact").unwrap().path, f.path("src/exact.ts"));
    assert_eq!(
        f.resolve(&r, "@lib/only").unwrap().path,
        f.path("vendor/only.ts")
    );
}

#[test]
fn import_map_resolves() {
    let f = fixture();
    let r = f.resolver(Some(tilde_map(&f)));
    assert_eq!(f.resolve(&r, "~/util").unwrap().path, f.path("src/util.ts"));
}

#[test]
fn base_url_is_the_fallback() {
    let f = fixture();
    assert_eq!(
        f.resolve(&f.resolver(None), "src/util").unwrap().path,
        f.path("src/util.ts")
    );
}

#[test]
fn unmatched_paths_target_is_not_found() {
    let f = fixture();
    let err = f.resolve(&f.resolver(None), "@/missing").unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[test]
fn target_outside_the_grant_is_never_statted() {
    let f = fixture();
    let err = f.resolve(&f.resolver(None), "escape/secret").unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
    assert!(
        err.message().contains("permitted roots"),
        "{}",
        err.message()
    );
    let outside = f.root.join("outside").to_string_lossy().into_owned();
    let calls = f.recording.calls.lock().unwrap();
    assert!(calls.iter().all(|(spec, _)| !spec.starts_with(&outside)));
}

#[test]
fn helper_module_is_not_shadowed_by_paths() {
    let f = fixture();
    let r = f.resolver(None);
    let _ = f.resolve(&r, "velyr-oxc-runtime/helpers/decorate");
    let calls = f.recording.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "velyr-oxc-runtime/helpers/decorate");
}

#[test]
fn node_modules_referrer_ignores_paths() {
    let f = fixture();
    let r = f.resolver(None);
    let referrer = f.referrer("node_modules/pkg/x.ts");
    let err = r
        .resolve("@/util", &referrer, &ResolveContext::default())
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[test]
fn caller_context_fields_survive_the_wrapper() {
    let f = fixture();
    let caller = ResolveContext {
        import_map: HashMap::from([("keep".to_owned(), "./keep.ts".to_owned())]),
        import_map_dir: f.path("caller-dir"),
        realpaths: Some(Default::default()),
    };
    let r = f.resolver(None);
    for spec in ["@/util", "./keep.ts", "keep"] {
        let _ = r.resolve(spec, &f.referrer("main.ts"), &caller);
    }
    let calls = f.recording.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    for (_, recorded) in calls.iter() {
        let ResolveContext {
            import_map,
            import_map_dir,
            realpaths,
        } = recorded.clone();
        assert!(std::sync::Arc::ptr_eq(
            realpaths.as_ref().unwrap(),
            caller.realpaths.as_ref().unwrap()
        ));
        assert_eq!(import_map, caller.import_map);
        assert_eq!(import_map_dir, caller.import_map_dir);
    }
}

#[test]
fn owned_import_map_replaces_the_callers_map_only() {
    let f = fixture();
    let r = f.resolver(Some(tilde_map(&f)));
    let caller = ResolveContext {
        import_map_dir: f.path("caller-dir"),
        ..Default::default()
    };
    r.resolve("~/util", &f.referrer("main.ts"), &caller)
        .unwrap();
    let calls = f.recording.calls.lock().unwrap();
    assert_eq!(calls[0].1.import_map_dir, f.path(""));
    assert_eq!(
        calls[0].1.import_map.get("~/").map(String::as_str),
        Some("./src/")
    );
}

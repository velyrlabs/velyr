//! ML-10: security properties of the per-epoch realpath cache (ML-03).
//!
//! The cache may serve a stale canonical path inside one epoch; that is only safe because
//! `authorize_path` re-resolves the path itself instead of trusting the resolver. These tests pin
//! that, plus epoch invalidation, `..` handling and the shape of errors for ungranted paths.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use velyr_runtime_errors::ErrorCode;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{
    FsResolver, ModuleId, ModuleResolver, RealpathCache, ResolveContext, ResolvedModule,
    authorize_path,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

/// `root/app` is granted; `root/secret` is not.
fn tree() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    for d in ["app/sub", "app/real", "secret/deep"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    for f in [
        "app/main.js",
        "app/a.js",
        "app/sub/b.js",
        "app/real/x.js",
        "secret/s.js",
        "secret/x.js",
    ] {
        fs::write(root.join(f), "export {};").unwrap();
    }
    (dir, root)
}

fn cached(cache: &Arc<RealpathCache>) -> ResolveContext {
    ResolveContext {
        realpaths: Some(Arc::clone(cache)),
        ..Default::default()
    }
}

fn grant_app(root: &Path) -> Arc<CapabilitySet> {
    let app = root.join("app").to_string_lossy().into_owned();
    CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(app)))
        .build()
}

fn resolve(
    root: &Path,
    specifier: &str,
    ctx: &ResolveContext,
) -> Result<ResolvedModule, VelyrError> {
    let referrer = ModuleId::from_path(&root.join("app/main.js")).unwrap();
    FsResolver.resolve(specifier, &referrer, ctx)
}

/// What a module load observes: resolution, then the permission check on the resolved path.
fn load_decision(
    root: &Path,
    specifier: &str,
    ctx: &ResolveContext,
) -> Result<PathBuf, VelyrError> {
    let module = resolve(root, specifier, ctx)?;
    authorize_path(&module.path, &grant_app(root))?;
    Ok(module.path)
}

#[cfg(unix)]
fn retarget(link: &Path, target: &Path) {
    fs::remove_file(link).unwrap();
    std::os::unix::fs::symlink(target, link).unwrap();
}

// (1) A parent symlink retargeted outside the grant is not served in the next epoch.
#[cfg(unix)]
#[test]
fn retargeted_parent_outside_the_grant_is_denied_in_the_next_epoch() {
    let (_d, root) = tree();
    let link = root.join("app/lib");
    std::os::unix::fs::symlink(root.join("app/real"), &link).unwrap();
    let cache = Arc::new(RealpathCache::default());
    let ctx = cached(&cache);
    assert_eq!(
        load_decision(&root, "./lib/x.js", &ctx).unwrap(),
        root.join("app/real/x.js")
    );

    retarget(&link, &root.join("secret"));
    cache.clear();
    let resolved = resolve(&root, "./lib/x.js", &ctx).unwrap();
    assert_eq!(resolved.path, root.join("secret/x.js"));
    let err = load_decision(&root, "./lib/x.js", &ctx).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PermissionDenied);
}

// (1)+(2) Inside one epoch the cache serves a stale parent, so the guarantee rests on
// `authorize_path` resolving the path again: a granted directory swapped for a symlink to an
// ungranted one must be denied even though the cached path still names the granted directory.
#[cfg(unix)]
#[test]
fn stale_parent_within_an_epoch_is_still_denied_by_the_permission_check() {
    let (_d, root) = tree();
    let lib = root.join("app/lib");
    fs::create_dir(&lib).unwrap();
    fs::write(lib.join("x.js"), "export {};").unwrap();
    let cache = Arc::new(RealpathCache::default());
    let ctx = cached(&cache);
    assert_eq!(
        load_decision(&root, "./lib/x.js", &ctx).unwrap(),
        lib.join("x.js")
    );

    fs::rename(&lib, root.join("app/lib_old")).unwrap();
    std::os::unix::fs::symlink(root.join("secret"), &lib).unwrap();
    let stale = resolve(&root, "./lib/x.js", &ctx).unwrap();
    assert_eq!(
        stale.path,
        lib.join("x.js"),
        "cached parent is reused inside the epoch"
    );
    let err = authorize_path(&stale.path, &grant_app(&root)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PermissionDenied);
}

// (2) The check runs on the canonical target, not on the specifier's granted-looking prefix.
#[cfg(unix)]
#[test]
fn symlink_alias_inside_the_grant_is_checked_on_its_target() {
    let (_d, root) = tree();
    std::os::unix::fs::symlink(root.join("secret"), root.join("app/evil")).unwrap();
    let cache = Arc::new(RealpathCache::default());
    for ctx in [ResolveContext::default(), cached(&cache), cached(&cache)] {
        assert_eq!(
            resolve(&root, "./evil/s.js", &ctx).unwrap().path,
            root.join("secret/s.js")
        );
        let err = load_decision(&root, "./evil/s.js", &ctx).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PermissionDenied);
    }
}

// (3) Keys never alias: `a/../b` resolves like the OS, including `..` after a symlink.
#[test]
fn dot_dot_specifiers_resolve_like_the_filesystem() {
    let (_d, root) = tree();
    let cache = Arc::new(RealpathCache::default());
    let ctx = cached(&cache);
    for spec in ["./a.js", "./sub/../a.js", "./sub/./../a.js"] {
        assert_eq!(
            resolve(&root, spec, &ctx).unwrap().path,
            root.join("app/a.js")
        );
    }
}

#[cfg(unix)]
#[test]
fn dot_dot_after_a_symlink_follows_the_target_and_is_denied() {
    let (_d, root) = tree();
    std::os::unix::fs::symlink(root.join("secret/deep"), root.join("app/jump")).unwrap();
    let cache = Arc::new(RealpathCache::default());
    let ctx = cached(&cache);
    let p = root.join("app/jump/../s.js");
    assert_eq!(
        cache.canonicalize(&p).unwrap(),
        dunce::canonicalize(&p).unwrap()
    );
    for _ in 0..2 {
        assert_eq!(
            resolve(&root, "./jump/../s.js", &ctx).unwrap().path,
            root.join("secret/s.js")
        );
        let err = load_decision(&root, "./jump/../s.js", &ctx).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PermissionDenied);
    }
}

// (4) Ungranted paths fail with the same code and message whether the parent is cached or not.
#[test]
fn ungranted_errors_have_the_same_shape_cached_and_uncached() {
    let (_d, root) = tree();
    let cache = Arc::new(RealpathCache::default());
    let warm = cached(&cache);
    let shape = |r: Result<PathBuf, VelyrError>| r.map_err(|e| (e.code(), e.to_string()));
    for spec in [
        "../secret/s.js",
        "../secret/missing.js",
        "../nowhere/x.js",
        "./sub/missing.js",
    ] {
        let cold = shape(load_decision(&root, spec, &ResolveContext::default()));
        let first = shape(load_decision(&root, spec, &warm));
        let second = shape(load_decision(&root, spec, &warm));
        assert!(cold.is_err(), "{spec} must not load");
        assert_eq!(cold, first, "{spec}");
        assert_eq!(cold, second, "{spec}");
    }
}

// Finding ML-10/F1: on a case-insensitive volume `fs::canonicalize` returns the on-disk case of
// the leaf, the cache keeps the caller's case, so one file gets two module identities.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "ML-10 finding F1: macOS leaf case is not canonicalized by RealpathCache"]
fn leaf_case_matches_fs_canonicalize() {
    let (_d, root) = tree();
    let p = root.join("app/A.JS");
    let cache = RealpathCache::default();
    assert_eq!(
        cache.canonicalize(&p).unwrap(),
        dunce::canonicalize(&p).unwrap()
    );
}

// Finding ML-10/F2: a trailing `/` or `/.` makes `symlink_metadata` follow a symlinked leaf, so
// the cache returns the link path instead of its target.
#[cfg(unix)]
#[test]
#[ignore = "ML-10 finding F2: trailing slash on a symlinked directory returns the link path"]
fn trailing_slash_on_a_symlinked_dir_matches_fs_canonicalize() {
    let (_d, root) = tree();
    std::os::unix::fs::symlink(root.join("secret"), root.join("app/dirlink")).unwrap();
    let cache = RealpathCache::default();
    for raw in ["app/dirlink/", "app/dirlink/."] {
        let p = PathBuf::from(format!("{}/{raw}", root.display()));
        assert_eq!(
            cache.canonicalize(&p).unwrap(),
            dunce::canonicalize(&p).unwrap(),
            "{raw}"
        );
    }
}

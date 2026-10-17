//! Loading `.node` files: `NativeLoad` authorization before `dlopen`, and both registration styles.
#![cfg(unix)] // dlopen-based checks (RTLD_NOLOAD) only exist on unix loaders.

mod support;

use std::path::{Path, PathBuf};

use velyr_compat_napi::{LoadError, load};
use velyr_engine_api::{HostError, NativeError, NativeScope, NativeValue, V8_ENGINE};
use velyr_runtime_errors::ErrorCode;
use velyr_runtime_permissions::CapabilitySet;

#[cfg(target_os = "macos")]
const RTLD_NOLOAD: i32 = 0x10;
#[cfg(not(target_os = "macos"))]
const RTLD_NOLOAD: i32 = 0x4;

fn is_loaded(path: &Path) -> bool {
    use libloading::os::unix::{Library, RTLD_LAZY};
    // SAFETY: RTLD_NOLOAD only queries the loader; it never maps the file or runs initializers.
    unsafe { Library::open(Some(path), RTLD_NOLOAD | RTLD_LAZY) }.is_ok()
}

/// A private copy of the addon, so its load state is not shared with other tests.
fn fresh_copy(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("napi-loader");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}-{}.node", std::process::id()));
    std::fs::copy(support::addon(), &path).unwrap();
    std::fs::canonicalize(path).unwrap()
}

fn load_error(path: &Path, permissions: &CapabilitySet) -> velyr_runtime_errors::VelyrError {
    support::with_context(|scope| match load(scope, path, permissions) {
        Err(LoadError::Velyr(err)) => err,
        Err(LoadError::Exception(_)) => panic!("unexpected JS exception"),
        Ok(_) => panic!("load unexpectedly succeeded"),
    })
}

#[test]
fn without_native_permission_the_library_is_never_opened() {
    let path = fresh_copy("denied");
    let err = load_error(&path, &CapabilitySet::default());
    assert_eq!(err.code(), ErrorCode::PermissionDenied);
    assert!(!is_loaded(&path), "dlopen ran before the permission check");
}

#[test]
fn permission_for_another_path_is_denied() {
    let path = fresh_copy("elsewhere");
    let other = support::allow_native(&path.with_file_name("other.node"));
    let err = load_error(&path, &other);
    assert_eq!(err.code(), ErrorCode::PermissionDenied);
    assert!(!is_loaded(&path));
}

#[test]
fn missing_file_is_not_found() {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("napi-loader/missing.node");
    let err = load_error(&path, &support::allow_native(&path));
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[test]
fn a_file_that_is_not_a_library_fails_cleanly() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("napi-loader");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("garbage.node");
    std::fs::write(&path, b"not a shared library").unwrap();
    let path = std::fs::canonicalize(path).unwrap();
    let err = load_error(&path, &support::allow_native(&path));
    assert_eq!(err.code(), ErrorCode::InvalidData);
}

#[test]
fn legacy_napi_module_register_addons_load_and_reload() {
    let _serial = support::serial();
    for _ in 0..2 {
        support::with_context(|scope| {
            support::install_addon(scope, support::legacy_addon());
            let result = support::run(
                scope,
                "assertEq(addon.legacy, 1, 'legacy init'); assertEq(addon.makeAdder(2)(3), 5, 'calls');",
            );
            velyr_compat_napi::shutdown(scope);
            result.unwrap();
        });
    }
}

#[test]
fn load_native_returns_the_exports_through_a_v8_native_scope() {
    let _serial = support::serial();
    let path = support::addon();
    support::with_context(|scope| {
        let mut lent = NativeScope::new(V8_ENGINE, std::ptr::NonNull::from(&mut *scope).cast());
        let exports = velyr_compat_napi::load_native(&mut lent, path, &support::allow_native(path))
            .expect("the addon loads");
        // SAFETY: `load_native` returns the bits of a `Local` created in `scope`.
        let exports =
            unsafe { std::mem::transmute::<NativeValue, v8::Local<'_, v8::Value>>(exports) };
        assert!(exports.is_object());
        velyr_compat_napi::shutdown_native(&mut NativeScope::new(
            V8_ENGINE,
            std::ptr::NonNull::from(&mut *scope).cast(),
        ));
    });
}

#[test]
fn load_native_refuses_a_scope_from_another_engine() {
    let path = support::addon();
    // Never dereferenced: the engine tag is checked first.
    let mut lent = NativeScope::new("native", std::ptr::NonNull::dangling());
    match velyr_compat_napi::load_native(&mut lent, path, &support::allow_native(path)) {
        Err(NativeError::Host(HostError::Velyr(err))) => {
            assert_eq!(err.code(), ErrorCode::Unsupported)
        }
        other => panic!("expected an unsupported error, got {other:?}"),
    }
}

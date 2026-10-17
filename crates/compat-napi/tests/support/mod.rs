//! Test harness: compiles the C addon in `tests/addons/`, boots V8 and runs JS against it.
#![allow(dead_code)] // each test binary uses a different part of the harness

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Once, OnceLock};

use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

const SOURCES: [&str; 9] = [
    "addon.c",
    "values.c",
    "strings.c",
    "objects.c",
    "functions.c",
    "classes.c",
    "errors.c",
    "buffers.c",
    "lifetime.c",
];

/// Counter slots of the addon (`enum counter` in `addon.h`).
pub const INSTANCE_DATA_FINALIZED: i32 = 0;
pub const WRAP_FINALIZED: i32 = 2;

/// The addon registered through `napi_register_module_v1`.
pub fn addon() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| compile("addon", &[]))
}

/// The same addon registered through the deprecated `napi_module_register` constructor.
pub fn legacy_addon() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| compile("legacy", &["LEGACY_REGISTER"]))
}

fn compile(name: &str, defines: &[&str]) -> PathBuf {
    let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/addons");
    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("napi-addons")
        .join(env!("CARGO_CRATE_NAME"));
    std::fs::create_dir_all(&out_dir).expect("create the addon output dir");
    let out = out_dir.join(format!("{name}.node"));
    let target = env!("VELYR_NAPI_TARGET");
    let compiler = cc::Build::new()
        .target(target)
        .host(target)
        .opt_level(0)
        .debug(false)
        .cargo_metadata(false)
        .get_compiler();
    let mut cmd = compiler.to_command();
    cmd.args(["-shared", "-fPIC", "-std=c11", "-Wall", "-Werror"])
        .arg("-I")
        .arg(sources.join("include"));
    if cfg!(target_os = "macos") {
        // Resolve `napi_*` in the host executable at load time, as node-gyp does.
        cmd.args(["-undefined", "dynamic_lookup"]);
    }
    for define in defines {
        cmd.arg(format!("-D{define}"));
    }
    for source in SOURCES {
        cmd.arg(sources.join(source));
    }
    let status = cmd
        .arg("-o")
        .arg(&out)
        .status()
        .expect("run the C compiler");
    assert!(
        status.success(),
        "compiling the test addon failed: {status}"
    );
    std::fs::canonicalize(out).expect("canonical addon path")
}

/// The addon keeps global C state (counters, a weak ref), so tests in one binary run one at a time.
pub fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn allow_native(path: &Path) -> Arc<CapabilitySet> {
    let pattern = PathPattern::new(path.to_str().expect("utf-8 path"));
    CapabilitySet::builder()
        .allow(Capability::NativeLoad(pattern))
        .build()
}

/// Runs `f` in a fresh isolate and context.
pub fn with_context<R>(f: impl FnOnce(&mut v8::PinScope<'_, '_>) -> R) -> R {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        v8::V8::set_flags_from_string("--expose-gc");
        v8::V8::initialize_platform(v8::new_default_platform(0, false).make_shared());
        v8::V8::initialize();
    });
    let isolate = &mut v8::Isolate::new(Default::default());
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    f(scope)
}

/// Loads `path` with native permission and binds its exports to `globalThis.addon`.
pub fn install_addon(scope: &mut v8::PinScope<'_, '_>, path: &Path) {
    let exports = match velyr_compat_napi::load(scope, path, &allow_native(path)) {
        Ok(exports) => exports,
        Err(err) => panic!("loading the addon failed: {err:?}"),
    };
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, "addon").unwrap();
    global.set(scope, key.into(), exports).unwrap();
}

const PRELUDE: &str = r#"
function assertEq(actual, expected, what) {
  if (!Object.is(actual, expected)) {
    throw new Error(`${what}: expected ${String(expected)}, got ${String(actual)}`);
  }
}
function assertThrows(fn, check, what) {
  try { fn(); } catch (e) {
    if (!check(e)) throw new Error(`${what}: unexpected ${String(e)}`);
    return;
  }
  throw new Error(`${what}: did not throw`);
}
"#;

/// Runs `code` after the assertion prelude; `Err` carries the uncaught exception text.
pub fn run(scope: &mut v8::PinScope<'_, '_>, code: &str) -> Result<(), String> {
    v8::tc_scope!(let tc, scope);
    let source = v8::String::new(tc, &format!("{PRELUDE}\n{code}")).unwrap();
    let completed = v8::Script::compile(tc, source, None).and_then(|script| script.run(tc));
    if completed.is_some() {
        return Ok(());
    }
    let exception = tc
        .exception()
        .map_or_else(|| "terminated".to_owned(), |e| e.to_rust_string_lossy(tc));
    Err(exception)
}

/// Loads the addon in a fresh context and runs `code` against it.
pub fn run_with_addon(code: &str) -> Result<(), String> {
    let _serial = serial();
    with_context(|scope| {
        install_addon(scope, addon());
        let result = run(scope, code);
        velyr_compat_napi::shutdown(scope);
        result
    })
}

/// Reads one of the addon's C counters directly, without going through JS.
pub fn counter(path: &Path, which: i32) -> i32 {
    // SAFETY: the addon is already loaded by the test; `velyr_test_counter` is a plain C getter
    // with this exact signature (tests/addons/addon.c).
    unsafe {
        let library = libloading::Library::new(path).expect("addon is loadable");
        let get: libloading::Symbol<'_, extern "C" fn(i32) -> i32> =
            library.get(b"velyr_test_counter").expect("counter getter");
        get(which)
    }
}

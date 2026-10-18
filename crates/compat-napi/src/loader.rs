//! Loading `.node` files: `NativeLoad` authorization, `dlopen` and module registration.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use libloading::Library;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_permissions::{CapabilitySet, Request};

use crate::env::{Env, EnvSlot};
use crate::ffi::{local, to_napi};
use crate::types::{NapiModule, napi_env, napi_value};

/// Modules that declare no version (or less) get this one, as in Node 24.
const DEFAULT_API_VERSION: i32 = 8;
/// Highest version a module may declare (`NODE_API_SUPPORTED_VERSION_MAX` in Node 24).
const MAX_API_VERSION: i32 = 10;

type RegisterFn = unsafe extern "C" fn(napi_env, napi_value) -> napi_value;
type GetApiVersionFn = unsafe extern "C" fn() -> i32;

/// A loaded library. Libraries are never unloaded (as in Node), so `register` stays valid.
struct Addon {
    register: RegisterFn,
    api_version: i32,
    _library: Library,
}

/// One entry per canonical path: a second `dlopen` would not rerun a legacy module's
/// registering constructor, so later loads reuse the first registration.
static ADDONS: LazyLock<Mutex<HashMap<PathBuf, Arc<Addon>>>> = LazyLock::new(Default::default);

thread_local! {
    /// Set by `napi_module_register` while a legacy module's constructors run inside `dlopen`.
    static PENDING_MODULE: Cell<*const NapiModule> = const { Cell::new(std::ptr::null()) };
}

/// Why [`load`] failed.
#[derive(Debug)]
pub enum LoadError<'s> {
    /// Denied, missing, not a library, unsupported version, or no registration.
    Velyr(VelyrError),
    /// The module's init threw this value.
    Exception(v8::Local<'s, v8::Value>),
}

/// Deprecated registration: legacy modules call this from a static constructor during `dlopen`.
#[unsafe(no_mangle)]
pub extern "C" fn napi_module_register(module: *mut NapiModule) {
    PENDING_MODULE.with(|pending| pending.set(module));
}

/// Canonicalizes `path` and checks `NativeLoad` for it. Never opens the library.
pub fn authorize(path: &Path, permissions: &CapabilitySet) -> Result<PathBuf, VelyrError> {
    let canonical = match dunce::canonicalize(path) {
        Ok(canonical) => canonical,
        Err(err) => {
            // Without permission for the path as written, do not reveal whether it exists.
            check(path, permissions)?;
            return Err(VelyrError::not_found("cannot find native addon").with_cause(err));
        }
    };
    check(&canonical, permissions)?;
    Ok(canonical)
}

fn check(path: &Path, permissions: &CapabilitySet) -> Result<(), VelyrError> {
    let text = path
        .to_str()
        .ok_or_else(|| VelyrError::invalid_data("native addon path is not valid UTF-8"))?;
    let decision = permissions.check(&Request::NativeLoad(text));
    if !decision.is_allow() {
        tracing::warn!(
            target: "velyr.permission.denied",
            op = "napi_load",
            kind = "native-load",
            target_len = text.len(),
            "velyr.permission.denied"
        );
    }
    decision.into_result()
}

fn open(canonical: &Path) -> Result<Arc<Addon>, VelyrError> {
    let mut addons = ADDONS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(addon) = addons.get(canonical) {
        return Ok(Arc::clone(addon));
    }
    PENDING_MODULE.with(|pending| pending.set(std::ptr::null()));
    // SAFETY: loading runs the addon's initializers, which is the point of a native addon; the
    // caller authorized `NativeLoad` for this canonical path.
    let library = unsafe { Library::new(canonical) }
        .map_err(|err| VelyrError::invalid_data(format!("cannot load native addon: {err}")))?;
    let legacy = PENDING_MODULE.with(|pending| pending.replace(std::ptr::null()));
    let (register, api_version) = if legacy.is_null() {
        (symbol_register(&library)?, declared_version(&library)?)
    } else {
        // SAFETY: `napi_module_register` received a pointer to the module's static `napi_module`.
        let register = unsafe { (*legacy).nm_register_func };
        (register.ok_or_else(not_registered)?, DEFAULT_API_VERSION)
    };
    let addon = Arc::new(Addon {
        register,
        api_version,
        _library: library,
    });
    addons.insert(canonical.to_owned(), Arc::clone(&addon));
    Ok(addon)
}

fn not_registered() -> VelyrError {
    VelyrError::invalid_data("native addon did not self-register")
}

fn symbol_register(library: &Library) -> Result<RegisterFn, VelyrError> {
    // SAFETY: `napi_register_module_v1` has this signature in every Node-API module.
    unsafe { library.get::<RegisterFn>(b"napi_register_module_v1") }
        .map(|symbol| *symbol)
        .map_err(|_| not_registered())
}

/// `node_api_module_get_api_version_v1`, clamped like Node: below 8 becomes 8, above 10 fails.
fn declared_version(library: &Library) -> Result<i32, VelyrError> {
    // SAFETY: the getter takes nothing and returns the module's `NAPI_VERSION`.
    let declared =
        match unsafe { library.get::<GetApiVersionFn>(b"node_api_module_get_api_version_v1") } {
            // SAFETY: calling the module's version getter, which has no preconditions.
            Ok(getter) => unsafe { getter() },
            Err(_) => DEFAULT_API_VERSION,
        };
    if declared > MAX_API_VERSION {
        return Err(VelyrError::unsupported(format!(
            "native addon requires Node-API version {declared}, but Velyr supports up to {MAX_API_VERSION}"
        )));
    }
    Ok(declared.max(DEFAULT_API_VERSION))
}

/// Loads the addon at `path` into the current context and returns its exports (what
/// `process.dlopen` assigns to `module.exports`). `NativeLoad` is checked before `dlopen`.
pub fn load<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    path: &Path,
    permissions: &CapabilitySet,
) -> Result<v8::Local<'s, v8::Value>, LoadError<'s>> {
    let canonical = authorize(path, permissions).map_err(LoadError::Velyr)?;
    let addon = open(&canonical).map_err(LoadError::Velyr)?;
    let env = Env::create(scope, addon.api_version);
    let exports: v8::Local<'_, v8::Value> = v8::Object::new(scope).into();
    // SAFETY: a module init receives a fresh env and the exports object, as in Node.
    let returned = unsafe { (addon.register)(env.raw(), to_napi(exports)) };
    // Keep the results alive across closing the scopes the init may have left open.
    let exception = env
        .take_pending_exception(scope)
        .map(|e| v8::Global::new(scope, e));
    let value = local(returned)
        .ok()
        .filter(|value| !value.is_null_or_undefined())
        .map(|value| v8::Global::new(scope, value));
    env.close_scopes_above(0);
    if let Some(exception) = exception {
        return Err(LoadError::Exception(v8::Local::new(scope, exception)));
    }
    Ok(value.map_or(exports, |value| v8::Local::new(scope, value)))
}

/// Tears down every env of this isolate: runs outstanding finalizers and instance-data
/// finalizers, then frees the envs. Call once, before disposing the isolate; addon functions
/// must not run afterwards. Without it the envs are freed with the isolate and no finalizer runs.
pub fn shutdown(scope: &mut v8::PinScope<'_, '_>) {
    let Some(EnvSlot(envs)) = scope.remove_slot::<EnvSlot>() else {
        return;
    };
    for env in envs.into_iter().rev() {
        env.teardown(scope);
    }
}

//! References, finalizers, externals, instance data and env-level queries.

use std::ffi::c_void;

use crate::env::{Env, Finalizer};
use crate::ffi::{local, local_as, napi_fn, napi_scope, to_napi, write};
use crate::types::{ExtendedErrorInfo, Status, napi_env, napi_finalize, napi_ref, napi_value};

/// What `napi_get_version` reports: the highest version whose surface this crate implements.
const SUPPORTED_API_VERSION: u32 = 8;

/// `napi_ref`: strong while `count > 0`, weak otherwise.
struct Reference {
    count: u32,
    strong: Option<v8::Global<v8::Value>>,
    weak: v8::Weak<v8::Value>,
}

/// Creates a reference with `count`; the caller owns it until `napi_delete_reference`.
// shortcut: references that are never deleted leak until process exit, as do Node's after env
// teardown; track them in the env if addons that leak many show up.
pub(crate) fn new_reference(
    isolate: &mut v8::Isolate,
    value: v8::Local<'_, v8::Value>,
    count: u32,
) -> napi_ref {
    let strong = (count > 0).then(|| v8::Global::new(isolate, value));
    let weak = v8::Weak::new(isolate, value);
    Box::into_raw(Box::new(Reference {
        count,
        strong,
        weak,
    }))
    .cast()
}

fn reference<'a>(reference: napi_ref) -> Result<&'a mut Reference, Status> {
    // SAFETY: a non-null `napi_ref` came from `new_reference` and was not deleted (N-API
    // contract); the env is single-threaded, so no other borrow is live.
    unsafe { reference.cast::<Reference>().as_mut() }.ok_or(Status::InvalidArg)
}

napi_fn! {
    fn napi_create_reference(env, value: napi_value, count: u32, result: *mut napi_ref) {
        napi_scope!(env, scope);
        let value = local(value)?;
        // Before Node-API 10 only objects, functions and symbols can be referenced.
        if env.module_api_version < 10 && !(value.is_object() || value.is_symbol()) {
            return Err(Status::InvalidArg);
        }
        if result.is_null() {
            return Err(Status::InvalidArg);
        }
        write(result, new_reference(scope, value, count))
    }
}

napi_fn! {
    fn napi_delete_reference(env, reference: napi_ref) {
        self::reference(reference)?;
        // SAFETY: checked non-null above; the addon gives up the reference here.
        drop(unsafe { Box::from_raw(reference.cast::<Reference>()) });
        Ok(())
    }
}

napi_fn! {
    fn napi_reference_ref(env, reference: napi_ref, result: *mut u32) {
        napi_scope!(env, scope);
        let reference = self::reference(reference)?;
        if reference.count == 0 {
            let value = reference.weak.to_local(scope).ok_or(Status::GenericFailure)?;
            reference.strong = Some(v8::Global::new(scope, value));
        }
        reference.count += 1;
        if !result.is_null() {
            write(result, reference.count)?;
        }
        Ok(())
    }
}

napi_fn! {
    fn napi_reference_unref(env, reference: napi_ref, result: *mut u32) {
        let reference = self::reference(reference)?;
        if reference.count == 0 {
            return Err(Status::GenericFailure);
        }
        reference.count -= 1;
        if reference.count == 0 {
            reference.strong = None;
        }
        if !result.is_null() {
            write(result, reference.count)?;
        }
        Ok(())
    }
}

napi_fn! {
    fn napi_get_reference_value(env, reference: napi_ref, result: *mut napi_value) {
        napi_scope!(env, scope);
        let reference = self::reference(reference)?;
        let value = match &reference.strong {
            Some(strong) => Some(v8::Local::new(scope, strong)),
            None => reference.weak.to_local(scope),
        };
        // A collected target reads as NULL, as in Node.
        write(result, value.map_or(std::ptr::null_mut(), to_napi))
    }
}

napi_fn! {
    fn napi_add_finalizer(
        env,
        target: napi_value,
        data: *mut c_void,
        finalize: napi_finalize,
        hint: *mut c_void,
        result: *mut napi_ref,
    ) {
        napi_scope!(env, scope);
        let target = local_as::<v8::Object>(target, Status::ObjectExpected)?;
        if finalize.is_none() {
            return Err(Status::InvalidArg);
        }
        env.track(scope, target.into(), Finalizer { callback: finalize, data, hint }, None);
        if !result.is_null() {
            write(result, new_reference(scope, target.into(), 0))?;
        }
        Ok(())
    }
}

napi_fn! {
    fn napi_create_external(
        env,
        data: *mut c_void,
        finalize: napi_finalize,
        hint: *mut c_void,
        result: *mut napi_value,
    ) {
        napi_scope!(env, scope);
        let external: v8::Local<'_, v8::Value> = v8::External::new(scope, data).into();
        if finalize.is_some() {
            env.track(scope, external, Finalizer { callback: finalize, data, hint }, None);
        }
        write(result, to_napi(external))
    }
}

napi_fn! {
    fn napi_get_value_external(env, value: napi_value, result: *mut *mut c_void) {
        write(result, local_as::<v8::External>(value, Status::InvalidArg)?.value())
    }
}

napi_fn! {
    fn napi_set_instance_data(env, data: *mut c_void, finalize: napi_finalize, hint: *mut c_void) {
        // As in Node, replacing instance data drops the old finalizer without running it.
        env.instance_data.set(Some(Finalizer { callback: finalize, data, hint }));
        Ok(())
    }
}

napi_fn! {
    fn napi_get_instance_data(env, result: *mut *mut c_void) {
        write(result, env.instance_data.get().map_or(std::ptr::null_mut(), |data| data.data))
    }
}

napi_fn! {
    fn napi_get_version(env, result: *mut u32) {
        write(result, SUPPORTED_API_VERSION)
    }
}

/// Written by hand: unlike every other call it must not overwrite the status it reports.
#[unsafe(no_mangle)]
pub extern "C" fn napi_get_last_error_info(
    env: napi_env,
    result: *mut *const ExtendedErrorInfo,
) -> Status {
    let Some(env) = Env::from_raw(env) else {
        return Status::InvalidArg;
    };
    write(result, env.error_info()).err().unwrap_or(Status::Ok)
}

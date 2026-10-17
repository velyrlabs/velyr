//! Native functions, callback info, calls and classes (`napi_define_class`, wrap/unwrap).

use std::ffi::{c_char, c_void};

use crate::api::objects::{define_properties, object};
use crate::env::{Finalizer, Wrap, arguments, callback_info};
use crate::ffi::{c_bytes, local, local_as, napi_fn, napi_scope, slice, slice_mut, to_napi, write};
use crate::types::{
    NAPI_STATIC, PropertyDescriptor, Status, napi_callback, napi_callback_info, napi_finalize,
    napi_ref, napi_value,
};

napi_fn! {
    fn napi_create_function(
        env,
        name: *const c_char,
        length: usize,
        callback: napi_callback,
        data: *mut c_void,
        result: *mut napi_value,
    ) {
        napi_scope!(env, scope);
        let name = if name.is_null() {
            None
        } else {
            let bytes = c_bytes(name, length)?;
            Some(v8::String::new_from_utf8(scope, bytes, v8::NewStringType::Normal).ok_or(Status::GenericFailure)?)
        };
        let function = env.function(scope, name, callback, data)?;
        write(result, to_napi(function.into()))
    }
}

napi_fn! {
    fn napi_get_cb_info(
        env,
        info: napi_callback_info,
        argc: *mut usize,
        argv: *mut napi_value,
        this_arg: *mut napi_value,
        data: *mut *mut c_void,
    ) {
        napi_scope!(env, scope);
        let info = callback_info(info)?;
        let args = arguments(info);
        let length = usize::try_from(args.length()).unwrap_or(0);
        if !argv.is_null() {
            // SAFETY: `argc` is required with `argv` and holds the capacity of `argv`.
            let capacity = unsafe { argc.as_ref() }.copied().ok_or(Status::InvalidArg)?;
            let undefined = to_napi(v8::undefined(scope).into());
            for (index, slot) in slice_mut(argv, capacity).unwrap_or_default().iter_mut().enumerate() {
                *slot = if index < length { to_napi(args.get(index as i32)) } else { undefined };
            }
        }
        if !argc.is_null() {
            write(argc, length)?;
        }
        if !this_arg.is_null() {
            write(this_arg, to_napi(args.this().into()))?;
        }
        if !data.is_null() {
            write(data, info.data)?;
        }
        Ok(())
    }
}

napi_fn! {
    fn napi_get_new_target(env, info: napi_callback_info, result: *mut napi_value) {
        let target = arguments(callback_info(info)?).new_target();
        write(result, if target.is_undefined() { std::ptr::null_mut() } else { to_napi(target) })
    }
}

fn argv_locals<'s>(
    argc: usize,
    argv: *const napi_value,
) -> Result<Vec<v8::Local<'s, v8::Value>>, Status> {
    slice(argv, argc)?
        .iter()
        .map(|value| local(*value))
        .collect()
}

napi_fn! {
    fn napi_call_function(
        env,
        recv: napi_value,
        function: napi_value,
        argc: usize,
        argv: *const napi_value,
        result: *mut napi_value,
    ) {
        napi_scope!(env, scope);
        let recv = local(recv)?;
        let function = local_as::<v8::Function>(function, Status::FunctionExpected)?;
        let args = argv_locals(argc, argv)?;
        let returned = env.run_js(scope, |scope| function.call(scope, recv, &args).map(to_napi))?;
        if !result.is_null() {
            write(result, returned)?;
        }
        Ok(())
    }
}

napi_fn! {
    fn napi_new_instance(
        env,
        constructor: napi_value,
        argc: usize,
        argv: *const napi_value,
        result: *mut napi_value,
    ) {
        napi_scope!(env, scope);
        let constructor = local_as::<v8::Function>(constructor, Status::FunctionExpected)?;
        let args = argv_locals(argc, argv)?;
        let instance =
            env.run_js(scope, |scope| constructor.new_instance(scope, &args).map(|o| to_napi(o.into())))?;
        write(result, instance)
    }
}

napi_fn! {
    fn napi_define_class(
        env,
        name: *const c_char,
        length: usize,
        constructor: napi_callback,
        data: *mut c_void,
        property_count: usize,
        properties: *const PropertyDescriptor,
        result: *mut napi_value,
    ) {
        napi_scope!(env, scope);
        let name = v8::String::new_from_utf8(scope, c_bytes(name, length)?, v8::NewStringType::Normal)
            .ok_or(Status::GenericFailure)?;
        let properties = slice(properties, property_count)?;
        let template = env.function_template(scope, constructor, data)?;
        template.set_class_name(name);
        let class = template.get_function(scope).ok_or(Status::GenericFailure)?;
        // Instance members go on `prototype` and statics on the constructor, defined like
        // `napi_define_properties` (Node uses template slots; the observable result matches).
        let prototype_key = v8::String::new(scope, "prototype").ok_or(Status::GenericFailure)?;
        let prototype = class.get(scope, prototype_key.into()).ok_or(Status::GenericFailure)?;
        let prototype = prototype.to_object(scope).ok_or(Status::GenericFailure)?;
        let is_static = |property: &&PropertyDescriptor| property.attributes & NAPI_STATIC != 0;
        define_properties(env, scope, prototype, properties.iter().filter(|p| !is_static(p)))?;
        define_properties(env, scope, class.into(), properties.iter().filter(is_static))?;
        write(result, to_napi(class.into()))
    }
}

fn wrap_key<'s>(scope: &v8::PinScope<'s, '_>) -> Result<v8::Local<'s, v8::Private>, Status> {
    let name = v8::String::new(scope, "velyr.napi.wrap").ok_or(Status::GenericFailure)?;
    Ok(v8::Private::for_api(scope, Some(name)))
}

/// The `Wrap` attached to `target`, if any.
fn find_wrap<'a>(
    scope: &v8::PinScope<'_, '_>,
    target: v8::Local<'_, v8::Object>,
) -> Result<Option<&'a Wrap>, Status> {
    let stored = target
        .get_private(scope, wrap_key(scope)?)
        .ok_or(Status::GenericFailure)?;
    let Ok(external) = v8::Local::<v8::External>::try_from(stored) else {
        return Ok(None);
    };
    // SAFETY: the private wrap key only ever holds an External over a `Wrap` owned by the
    // object's tracked entry, which lives as long as the object (or until `remove_wrap`).
    Ok(Some(unsafe { &*external.value().cast::<Wrap>() }))
}

napi_fn! {
    fn napi_wrap(
        env,
        target: napi_value,
        native: *mut c_void,
        finalize: napi_finalize,
        hint: *mut c_void,
        result: *mut napi_ref,
    ) {
        napi_scope!(env, scope);
        let target = local_as::<v8::Object>(target, Status::ObjectExpected)?;
        if find_wrap(scope, target)?.is_some() {
            return Err(Status::InvalidArg);
        }
        let wrap = Box::new(Wrap { data: native, id: 0 });
        let external = v8::External::new(scope, (&raw const *wrap).cast_mut().cast());
        target.set_private(scope, wrap_key(scope)?, external.into()).ok_or(Status::GenericFailure)?;
        let finalizer = Finalizer { callback: finalize, data: native, hint };
        env.track(scope, target.into(), finalizer, Some(wrap));
        if !result.is_null() {
            write(result, crate::api::lifetime::new_reference(scope, target.into(), 0))?;
        }
        Ok(())
    }
}

napi_fn! {
    fn napi_unwrap(env, target: napi_value, result: *mut *mut c_void) {
        napi_scope!(env, scope);
        let target = local_as::<v8::Object>(target, Status::ObjectExpected)?;
        let wrap = find_wrap(scope, target)?.ok_or(Status::InvalidArg)?;
        write(result, wrap.data)
    }
}

napi_fn! {
    fn napi_remove_wrap(env, target: napi_value, result: *mut *mut c_void) {
        napi_scope!(env, scope);
        let target = object(scope, target)?;
        let wrap = find_wrap(scope, target)?.ok_or(Status::InvalidArg)?;
        let (data, id) = (wrap.data, wrap.id);
        target.delete_private(scope, wrap_key(scope)?).ok_or(Status::GenericFailure)?;
        // Frees the `Wrap` and cancels its finalizer: the addon took the data back.
        env.untrack(id);
        if !result.is_null() {
            write(result, data)?;
        }
        Ok(())
    }
}

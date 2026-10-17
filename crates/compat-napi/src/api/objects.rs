//! Objects, properties, property descriptors and arrays.

use std::ffi::c_char;

use crate::env::Env;
use crate::ffi::{c_name, local, local_as, napi_fn, napi_scope, slice, to_napi, write};
use crate::types::{
    NAPI_CONFIGURABLE, NAPI_ENUMERABLE, NAPI_WRITABLE, PropertyDescriptor, Status, napi_value,
};

/// `ToObject` for an argument, as Node's `CHECK_TO_OBJECT`: null and undefined are rejected
/// before converting, so the conversion cannot throw.
pub(crate) fn object<'s>(
    scope: &v8::PinScope<'s, '_>,
    value: napi_value,
) -> Result<v8::Local<'s, v8::Object>, Status> {
    let value = local(value)?;
    if value.is_null_or_undefined() {
        return Err(Status::ObjectExpected);
    }
    value.to_object(scope).ok_or(Status::ObjectExpected)
}

fn utf8<'s>(
    scope: &v8::PinScope<'s, '_>,
    name: *const c_char,
) -> Result<v8::Local<'s, v8::String>, Status> {
    v8::String::new_from_utf8(scope, c_name(name)?, v8::NewStringType::Internalized)
        .ok_or(Status::GenericFailure)
}

napi_fn! {
    fn napi_create_object(env, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(v8::Object::new(scope).into()))
    }
}

napi_fn! {
    fn napi_set_property(env, target: napi_value, key: napi_value, value: napi_value) {
        napi_scope!(env, scope);
        let (target, key, value) = (object(scope, target)?, local(key)?, local(value)?);
        env.run_js(scope, |scope| target.set(scope, key, value)).map(drop)
    }
}

napi_fn! {
    fn napi_get_property(env, target: napi_value, key: napi_value, result: *mut napi_value) {
        napi_scope!(env, scope);
        let (target, key) = (object(scope, target)?, local(key)?);
        write(result, env.run_js(scope, |scope| target.get(scope, key).map(to_napi))?)
    }
}

napi_fn! {
    fn napi_has_property(env, target: napi_value, key: napi_value, result: *mut bool) {
        napi_scope!(env, scope);
        let (target, key) = (object(scope, target)?, local(key)?);
        write(result, env.run_js(scope, |scope| target.has(scope, key))?)
    }
}

napi_fn! {
    fn napi_has_own_property(env, target: napi_value, key: napi_value, result: *mut bool) {
        napi_scope!(env, scope);
        let target = object(scope, target)?;
        let key = local_as::<v8::Name>(key, Status::NameExpected)?;
        write(result, env.run_js(scope, |scope| target.has_own_property(scope, key))?)
    }
}

napi_fn! {
    fn napi_delete_property(env, target: napi_value, key: napi_value, result: *mut bool) {
        napi_scope!(env, scope);
        let (target, key) = (object(scope, target)?, local(key)?);
        let deleted = env.run_js(scope, |scope| target.delete(scope, key))?;
        if !result.is_null() {
            write(result, deleted)?;
        }
        Ok(())
    }
}

napi_fn! {
    fn napi_set_named_property(env, target: napi_value, name: *const c_char, value: napi_value) {
        napi_scope!(env, scope);
        let (target, value) = (object(scope, target)?, local(value)?);
        let key = utf8(scope, name)?;
        env.run_js(scope, |scope| target.set(scope, key.into(), value)).map(drop)
    }
}

napi_fn! {
    fn napi_get_named_property(env, target: napi_value, name: *const c_char, result: *mut napi_value) {
        napi_scope!(env, scope);
        let target = object(scope, target)?;
        let key = utf8(scope, name)?;
        write(result, env.run_js(scope, |scope| target.get(scope, key.into()).map(to_napi))?)
    }
}

napi_fn! {
    fn napi_has_named_property(env, target: napi_value, name: *const c_char, result: *mut bool) {
        napi_scope!(env, scope);
        let target = object(scope, target)?;
        let key = utf8(scope, name)?;
        write(result, env.run_js(scope, |scope| target.has(scope, key.into()))?)
    }
}

napi_fn! {
    fn napi_get_property_names(env, target: napi_value, result: *mut napi_value) {
        napi_scope!(env, scope);
        let target = object(scope, target)?;
        // Node: for-in names (prototypes included, enumerable, no symbols), indices as strings.
        let args = v8::GetPropertyNamesArgs {
            key_conversion: v8::KeyConversionMode::ConvertToString,
            ..Default::default()
        };
        let names = env.run_js(scope, |scope| target.get_property_names(scope, args).map(|n| to_napi(n.into())))?;
        write(result, names)
    }
}

/// Applies Node-API property descriptors to `target`; shared by `napi_define_class`.
pub(crate) fn define_properties<'a>(
    env: &Env,
    scope: &mut v8::PinScope<'_, '_>,
    target: v8::Local<'_, v8::Object>,
    properties: impl IntoIterator<Item = &'a PropertyDescriptor>,
) -> Result<(), Status> {
    for property in properties {
        let key: v8::Local<'_, v8::Name> = if property.utf8name.is_null() {
            local_as::<v8::Name>(property.name, Status::NameExpected)?
        } else {
            utf8(scope, property.utf8name)?.into()
        };
        let enumerable = property.attributes & NAPI_ENUMERABLE != 0;
        let configurable = property.attributes & NAPI_CONFIGURABLE != 0;
        let writable = property.attributes & NAPI_WRITABLE != 0;
        let mut descriptor = if property.getter.is_some() || property.setter.is_some() {
            let undefined: v8::Local<'_, v8::Value> = v8::undefined(scope).into();
            let accessor = |callback| match callback {
                Some(_) => env
                    .function(scope, None, callback, property.data)
                    .map(Into::into),
                None => Ok(undefined),
            };
            let (getter, setter) = (accessor(property.getter)?, accessor(property.setter)?);
            v8::PropertyDescriptor::new_from_get_set(getter, setter)
        } else if property.method.is_some() {
            let name = key.try_into().ok();
            let method = env.function(scope, name, property.method, property.data)?;
            v8::PropertyDescriptor::new_from_value_writable(method.into(), writable)
        } else {
            v8::PropertyDescriptor::new_from_value_writable(local(property.value)?, writable)
        };
        descriptor.set_enumerable(enumerable);
        descriptor.set_configurable(configurable);
        env.run_js(scope, |scope| {
            target.define_property(scope, key, &descriptor)
        })?;
    }
    Ok(())
}

napi_fn! {
    fn napi_define_properties(
        env,
        target: napi_value,
        count: usize,
        properties: *const PropertyDescriptor,
    ) {
        napi_scope!(env, scope);
        let target = object(scope, target)?;
        define_properties(env, scope, target, slice(properties, count)?)
    }
}

napi_fn! {
    fn napi_get_prototype(env, target: napi_value, result: *mut napi_value) {
        napi_scope!(env, scope);
        let prototype = object(scope, target)?.get_prototype(scope).ok_or(Status::GenericFailure)?;
        write(result, to_napi(prototype))
    }
}

napi_fn! {
    fn napi_instanceof(env, target: napi_value, constructor: napi_value, result: *mut bool) {
        napi_scope!(env, scope);
        let target = local(target)?;
        let constructor = object(scope, constructor)?;
        if !constructor.is_function() {
            let message = v8::String::new(scope, "Constructor must be a function").ok_or(Status::GenericFailure)?;
            env.set_pending_exception(scope, v8::Exception::type_error(scope, message));
            return Err(Status::FunctionExpected);
        }
        write(result, env.run_js(scope, |scope| target.instance_of(scope, constructor))?)
    }
}

fn array(value: napi_value) -> Result<v8::Local<'static, v8::Array>, Status> {
    local_as::<v8::Array>(value, Status::ArrayExpected)
}

napi_fn! {
    fn napi_create_array(env, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(v8::Array::new(scope, 0).into()))
    }
}

napi_fn! {
    fn napi_create_array_with_length(env, length: usize, result: *mut napi_value) {
        napi_scope!(env, scope);
        // As in Node, lengths beyond i32 are clamped rather than rejected.
        let length = i32::try_from(length).unwrap_or(i32::MAX);
        write(result, to_napi(v8::Array::new(scope, length).into()))
    }
}

napi_fn! {
    fn napi_get_array_length(env, value: napi_value, result: *mut u32) {
        write(result, array(value)?.length())
    }
}

napi_fn! {
    fn napi_is_array(env, value: napi_value, result: *mut bool) {
        write(result, local(value)?.is_array())
    }
}

napi_fn! {
    fn napi_set_element(env, target: napi_value, index: u32, value: napi_value) {
        napi_scope!(env, scope);
        let (target, value) = (object(scope, target)?, local(value)?);
        env.run_js(scope, |scope| target.set_index(scope, index, value)).map(drop)
    }
}

napi_fn! {
    fn napi_get_element(env, target: napi_value, index: u32, result: *mut napi_value) {
        napi_scope!(env, scope);
        let target = object(scope, target)?;
        write(result, env.run_js(scope, |scope| target.get_index(scope, index).map(to_napi))?)
    }
}

napi_fn! {
    fn napi_has_element(env, target: napi_value, index: u32, result: *mut bool) {
        napi_scope!(env, scope);
        let target = object(scope, target)?;
        write(result, env.run_js(scope, |scope| target.has_index(scope, index))?)
    }
}

napi_fn! {
    fn napi_delete_element(env, target: napi_value, index: u32, result: *mut bool) {
        napi_scope!(env, scope);
        let target = object(scope, target)?;
        let deleted = env.run_js(scope, |scope| target.delete_index(scope, index))?;
        if !result.is_null() {
            write(result, deleted)?;
        }
        Ok(())
    }
}

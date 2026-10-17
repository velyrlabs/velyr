//! Primitive values, `typeof`, strict equality and coercion.

use crate::ffi::{local, local_as, napi_fn, napi_scope, to_napi, write};
use crate::types::{Status, ValueType, napi_value};

napi_fn! {
    fn napi_get_undefined(env, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(v8::undefined(scope).into()))
    }
}

napi_fn! {
    fn napi_get_null(env, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(v8::null(scope).into()))
    }
}

napi_fn! {
    fn napi_get_global(env, result: *mut napi_value) {
        napi_scope!(env, scope);
        let global = scope.get_current_context().global(scope);
        write(result, to_napi(global.into()))
    }
}

napi_fn! {
    fn napi_get_boolean(env, value: bool, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(v8::Boolean::new(scope, value).into()))
    }
}

napi_fn! {
    fn napi_create_double(env, value: f64, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(v8::Number::new(scope, value).into()))
    }
}

napi_fn! {
    fn napi_create_int32(env, value: i32, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(v8::Integer::new(scope, value).into()))
    }
}

napi_fn! {
    fn napi_create_uint32(env, value: u32, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(v8::Integer::new_from_unsigned(scope, value).into()))
    }
}

napi_fn! {
    fn napi_create_int64(env, value: i64, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(v8::Number::new(scope, value as f64).into()))
    }
}

napi_fn! {
    fn napi_create_bigint_int64(env, value: i64, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(v8::BigInt::new_from_i64(scope, value).into()))
    }
}

napi_fn! {
    fn napi_create_symbol(env, description: napi_value, result: *mut napi_value) {
        napi_scope!(env, scope);
        let description = if description.is_null() {
            None
        } else {
            Some(local_as::<v8::String>(description, Status::StringExpected)?)
        };
        write(result, to_napi(v8::Symbol::new(scope, description).into()))
    }
}

fn number(value: napi_value) -> Result<v8::Local<'static, v8::Number>, Status> {
    local_as::<v8::Number>(value, Status::NumberExpected)
}

napi_fn! {
    fn napi_get_value_double(env, value: napi_value, result: *mut f64) {
        write(result, number(value)?.value())
    }
}

napi_fn! {
    fn napi_get_value_int32(env, value: napi_value, result: *mut i32) {
        napi_scope!(env, scope);
        // ToInt32 on a number never runs JS: NaN and infinities become 0.
        write(result, number(value)?.int32_value(scope).unwrap_or(0))
    }
}

napi_fn! {
    fn napi_get_value_uint32(env, value: napi_value, result: *mut u32) {
        napi_scope!(env, scope);
        write(result, number(value)?.uint32_value(scope).unwrap_or(0))
    }
}

napi_fn! {
    fn napi_get_value_int64(env, value: napi_value, result: *mut i64) {
        napi_scope!(env, scope);
        let number = number(value)?;
        let integer = if number.value().is_finite() { number.integer_value(scope).unwrap_or(0) } else { 0 };
        write(result, integer)
    }
}

napi_fn! {
    fn napi_get_value_bool(env, value: napi_value, result: *mut bool) {
        write(result, local_as::<v8::Boolean>(value, Status::BooleanExpected)?.is_true())
    }
}

napi_fn! {
    fn napi_get_value_bigint_int64(env, value: napi_value, result: *mut i64, lossless: *mut bool) {
        let (number, exact) = local_as::<v8::BigInt>(value, Status::BigintExpected)?.i64_value();
        write(lossless, exact)?;
        write(result, number)
    }
}

napi_fn! {
    fn napi_typeof(env, value: napi_value, result: *mut ValueType) {
        let value = local(value)?;
        // Same order as Node: externals are objects, so they are checked first.
        let kind = if value.is_number() {
            ValueType::Number
        } else if value.is_big_int() {
            ValueType::Bigint
        } else if value.is_string() {
            ValueType::String
        } else if value.is_function() {
            ValueType::Function
        } else if value.is_external() {
            ValueType::External
        } else if value.is_object() {
            ValueType::Object
        } else if value.is_boolean() {
            ValueType::Boolean
        } else if value.is_undefined() {
            ValueType::Undefined
        } else if value.is_symbol() {
            ValueType::Symbol
        } else if value.is_null() {
            ValueType::Null
        } else {
            return Err(Status::InvalidArg);
        };
        write(result, kind)
    }
}

napi_fn! {
    fn napi_strict_equals(env, lhs: napi_value, rhs: napi_value, result: *mut bool) {
        write(result, local(lhs)?.strict_equals(local(rhs)?))
    }
}

napi_fn! {
    fn napi_coerce_to_bool(env, value: napi_value, result: *mut napi_value) {
        napi_scope!(env, scope);
        write(result, to_napi(local(value)?.to_boolean(scope).into()))
    }
}

napi_fn! {
    fn napi_coerce_to_number(env, value: napi_value, result: *mut napi_value) {
        napi_scope!(env, scope);
        let value = local(value)?;
        let number = env.run_js(scope, |scope| value.to_number(scope).map(|n| to_napi(n.into())))?;
        write(result, number)
    }
}

napi_fn! {
    fn napi_coerce_to_string(env, value: napi_value, result: *mut napi_value) {
        napi_scope!(env, scope);
        let value = local(value)?;
        let string = env.run_js(scope, |scope| value.to_string(scope).map(|s| to_napi(s.into())))?;
        write(result, string)
    }
}

napi_fn! {
    fn napi_coerce_to_object(env, value: napi_value, result: *mut napi_value) {
        napi_scope!(env, scope);
        let value = local(value)?;
        let object = env.run_js(scope, |scope| value.to_object(scope).map(|o| to_napi(o.into())))?;
        write(result, object)
    }
}

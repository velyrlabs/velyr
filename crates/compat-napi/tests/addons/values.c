/* Primitive values, typeof, equality and coercion. */
#include "addon.h"

#define SET(env, object, name, expr)                                           \
  do {                                                                         \
    napi_value value_;                                                         \
    NAPI_CALL((env), (expr));                                                  \
    NAPI_CALL((env), napi_set_named_property((env), (object), (name), value_)); \
  } while (0)

static napi_value primitives(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value result, description;
  NAPI_CALL(env, napi_create_object(env, &result));
  SET(env, result, "undef", napi_get_undefined(env, &value_));
  SET(env, result, "nul", napi_get_null(env, &value_));
  SET(env, result, "yes", napi_get_boolean(env, true, &value_));
  SET(env, result, "no", napi_get_boolean(env, false, &value_));
  SET(env, result, "dbl", napi_create_double(env, 1.5, &value_));
  SET(env, result, "i32", napi_create_int32(env, -7, &value_));
  SET(env, result, "u32", napi_create_uint32(env, 4000000000u, &value_));
  SET(env, result, "i64", napi_create_int64(env, 9007199254740991LL, &value_));
  SET(env, result, "big", napi_create_bigint_int64(env, -5, &value_));
  SET(env, result, "global", napi_get_global(env, &value_));
  NAPI_CALL(env, napi_create_string_utf8(env, "tag", NAPI_AUTO_LENGTH, &description));
  SET(env, result, "sym", napi_create_symbol(env, description, &value_));
  return result;
}

static napi_value describe(napi_env env, napi_callback_info info) {
  static const char* names[] = {"undefined", "null",     "boolean",  "number", "string",
                                "symbol",    "object",   "function", "external", "bigint"};
  GET_ARGS(env, info, 1);
  napi_valuetype type;
  NAPI_CALL(env, napi_typeof(env, args[0], &type));
  napi_value result;
  NAPI_CALL(env, napi_create_string_utf8(env, names[type], NAPI_AUTO_LENGTH, &result));
  return result;
}

static napi_value numbers(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  double d;
  int32_t i32;
  uint32_t u32;
  int64_t i64;
  NAPI_CALL(env, napi_get_value_double(env, args[0], &d));
  NAPI_CALL(env, napi_get_value_int32(env, args[0], &i32));
  NAPI_CALL(env, napi_get_value_uint32(env, args[0], &u32));
  NAPI_CALL(env, napi_get_value_int64(env, args[0], &i64));
  napi_value result;
  NAPI_CALL(env, napi_create_object(env, &result));
  SET(env, result, "double", napi_create_double(env, d, &value_));
  SET(env, result, "int32", napi_create_int32(env, i32, &value_));
  SET(env, result, "uint32", napi_create_uint32(env, u32, &value_));
  SET(env, result, "int64", napi_create_int64(env, i64, &value_));
  return result;
}

static napi_value negate(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  bool value;
  NAPI_CALL(env, napi_get_value_bool(env, args[0], &value));
  napi_value result;
  NAPI_CALL(env, napi_get_boolean(env, !value, &result));
  return result;
}

static napi_value bigint_add_one(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  int64_t value;
  bool lossless;
  NAPI_CALL(env, napi_get_value_bigint_int64(env, args[0], &value, &lossless));
  napi_value result;
  NAPI_CALL(env, napi_create_bigint_int64(env, value + 1, &result));
  return result;
}

static napi_value strict_equals(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 2);
  bool equal;
  NAPI_CALL(env, napi_strict_equals(env, args[0], args[1], &equal));
  napi_value result;
  NAPI_CALL(env, napi_get_boolean(env, equal, &result));
  return result;
}

static napi_value coerce(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  napi_value result, item;
  NAPI_CALL(env, napi_create_array_with_length(env, 4, &result));
  NAPI_CALL(env, napi_coerce_to_string(env, args[0], &item));
  NAPI_CALL(env, napi_set_element(env, result, 0, item));
  NAPI_CALL(env, napi_coerce_to_number(env, args[0], &item));
  NAPI_CALL(env, napi_set_element(env, result, 1, item));
  NAPI_CALL(env, napi_coerce_to_bool(env, args[0], &item));
  NAPI_CALL(env, napi_set_element(env, result, 2, item));
  NAPI_CALL(env, napi_coerce_to_object(env, args[0], &item));
  NAPI_CALL(env, napi_set_element(env, result, 3, item));
  return result;
}

napi_value register_values(napi_env env, napi_value exports) {
  NAPI_CALL(env, export_function(env, exports, "primitives", primitives));
  NAPI_CALL(env, export_function(env, exports, "describe", describe));
  NAPI_CALL(env, export_function(env, exports, "numbers", numbers));
  NAPI_CALL(env, export_function(env, exports, "negate", negate));
  NAPI_CALL(env, export_function(env, exports, "bigintAddOne", bigint_add_one));
  NAPI_CALL(env, export_function(env, exports, "strictEquals", strict_equals));
  NAPI_CALL(env, export_function(env, exports, "coerce", coerce));
  return exports;
}

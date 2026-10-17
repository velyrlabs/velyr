/* Errors and exceptions. */
#include <stdio.h>

#include "addon.h"

static napi_value throw_error(napi_env env, napi_callback_info info) {
  (void)info;
  napi_throw_error(env, "E_ADDON", "plain failure");
  return NULL;
}

static napi_value throw_type_error(napi_env env, napi_callback_info info) {
  (void)info;
  napi_throw_type_error(env, "E_TYPE", "bad type");
  return NULL;
}

static napi_value throw_range_error(napi_env env, napi_callback_info info) {
  (void)info;
  napi_throw_range_error(env, NULL, "out of range");
  return NULL;
}

static napi_value throw_value(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  NAPI_CALL(env, napi_throw(env, args[0]));
  return NULL;
}

/* createErrors(): [Error, TypeError, RangeError], each with code "E_CREATED". */
static napi_value create_errors(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value code, message, result, error;
  NAPI_CALL(env, napi_create_string_utf8(env, "E_CREATED", NAPI_AUTO_LENGTH, &code));
  NAPI_CALL(env, napi_create_string_utf8(env, "created", NAPI_AUTO_LENGTH, &message));
  NAPI_CALL(env, napi_create_array(env, &result));
  NAPI_CALL(env, napi_create_error(env, code, message, &error));
  NAPI_CALL(env, napi_set_element(env, result, 0, error));
  NAPI_CALL(env, napi_create_type_error(env, code, message, &error));
  NAPI_CALL(env, napi_set_element(env, result, 1, error));
  NAPI_CALL(env, napi_create_range_error(env, code, message, &error));
  NAPI_CALL(env, napi_set_element(env, result, 2, error));
  return result;
}

static napi_value is_error(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  bool flag;
  NAPI_CALL(env, napi_is_error(env, args[0], &flag));
  napi_value result;
  NAPI_CALL(env, napi_get_boolean(env, flag, &result));
  return result;
}

/* catchFrom(fn): calls fn, then reports {status, pending, cleared, exception}. */
static napi_value catch_from(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  napi_value global, ignored, exception, report;
  NAPI_CALL(env, napi_get_global(env, &global));
  napi_status status = napi_call_function(env, global, args[0], 0, NULL, &ignored);
  bool pending = false, cleared = true;
  napi_is_exception_pending(env, &pending);
  NAPI_CALL(env, napi_get_and_clear_last_exception(env, &exception));
  NAPI_CALL(env, napi_is_exception_pending(env, &cleared));
  NAPI_CALL(env, napi_create_object(env, &report));
  NAPI_CALL(env, set_named_int(env, report, "status", (int32_t)status));
  NAPI_CALL(env, set_named_int(env, report, "pending", pending));
  NAPI_CALL(env, set_named_int(env, report, "stillPending", cleared));
  NAPI_CALL(env, napi_set_named_property(env, report, "exception", exception));
  return report;
}

/* lastError(): the extended info after a deliberate napi_number_expected failure. */
static napi_value last_error(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value text, report, message;
  double ignored;
  NAPI_CALL(env, napi_create_string_utf8(env, "nan", NAPI_AUTO_LENGTH, &text));
  napi_status status = napi_get_value_double(env, text, &ignored);
  const napi_extended_error_info* error_info;
  NAPI_CALL(env, napi_get_last_error_info(env, &error_info));
  NAPI_CALL(env, napi_create_object(env, &report));
  NAPI_CALL(env, set_named_int(env, report, "status", (int32_t)status));
  NAPI_CALL(env, set_named_int(env, report, "code", (int32_t)error_info->error_code));
  NAPI_CALL(env, napi_create_string_utf8(env, error_info->error_message, NAPI_AUTO_LENGTH,
                                         &message));
  NAPI_CALL(env, napi_set_named_property(env, report, "message", message));
  return report;
}

napi_value register_errors(napi_env env, napi_value exports) {
  NAPI_CALL(env, export_function(env, exports, "throwError", throw_error));
  NAPI_CALL(env, export_function(env, exports, "throwTypeError", throw_type_error));
  NAPI_CALL(env, export_function(env, exports, "throwRangeError", throw_range_error));
  NAPI_CALL(env, export_function(env, exports, "throwValue", throw_value));
  NAPI_CALL(env, export_function(env, exports, "createErrors", create_errors));
  NAPI_CALL(env, export_function(env, exports, "isError", is_error));
  NAPI_CALL(env, export_function(env, exports, "catchFrom", catch_from));
  NAPI_CALL(env, export_function(env, exports, "lastError", last_error));
  return exports;
}

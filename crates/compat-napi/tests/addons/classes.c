/* Classes (napi_define_class) with wrap, unwrap and remove_wrap. */
#include <stdlib.h>

#include "addon.h"

typedef struct {
  int32_t value;
} counter_t;

static void finalize_counter(napi_env env, void* data, void* hint) {
  (void)env;
  (void)hint;
  free(data);
  counters[COUNTER_WRAP_FINALIZED]++;
}

static napi_value counter_new(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value args[1], self, target;
  NAPI_CALL(env, napi_get_cb_info(env, info, &argc, args, &self, NULL));
  NAPI_CALL(env, napi_get_new_target(env, info, &target));
  if (target == NULL) {
    napi_throw_type_error(env, NULL, "Counter must be called with new");
    return NULL;
  }
  counter_t* counter = calloc(1, sizeof *counter);
  if (argc >= 1) NAPI_CALL(env, napi_get_value_int32(env, args[0], &counter->value));
  napi_ref ref;
  NAPI_CALL(env, napi_wrap(env, self, counter, finalize_counter, NULL, &ref));
  NAPI_CALL(env, napi_delete_reference(env, ref));
  return NULL;
}

static napi_value unwrap_this(napi_env env, napi_callback_info info, napi_value* arg,
                              counter_t** counter) {
  size_t argc = 1;
  napi_value self;
  NAPI_CALL(env, napi_get_cb_info(env, info, &argc, arg, &self, NULL));
  NAPI_CALL(env, napi_unwrap(env, self, (void**)counter));
  return self;
}

static napi_value counter_increment(napi_env env, napi_callback_info info) {
  counter_t* counter;
  napi_value arg;
  if (unwrap_this(env, info, &arg, &counter) == NULL) return NULL;
  napi_value result;
  NAPI_CALL(env, napi_create_int32(env, ++counter->value, &result));
  return result;
}

static napi_value counter_get(napi_env env, napi_callback_info info) {
  counter_t* counter;
  napi_value arg;
  if (unwrap_this(env, info, &arg, &counter) == NULL) return NULL;
  napi_value result;
  NAPI_CALL(env, napi_create_int32(env, counter->value, &result));
  return result;
}

static napi_value counter_set(napi_env env, napi_callback_info info) {
  counter_t* counter;
  napi_value arg;
  if (unwrap_this(env, info, &arg, &counter) == NULL) return NULL;
  NAPI_CALL(env, napi_get_value_int32(env, arg, &counter->value));
  return NULL;
}

static napi_value counter_kind(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value result;
  NAPI_CALL(env, napi_create_string_utf8(env, "counter", NAPI_AUTO_LENGTH, &result));
  return result;
}

/* detach(counter): removes the wrap and returns the last value; the finalizer must not run. */
static napi_value detach(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  counter_t* counter;
  NAPI_CALL(env, napi_remove_wrap(env, args[0], (void**)&counter));
  int32_t value = counter->value;
  free(counter);
  napi_value result;
  NAPI_CALL(env, napi_create_int32(env, value, &result));
  return result;
}

/* wrapTwice(obj): the status of a second napi_wrap on the same object. */
static napi_value wrap_twice(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  static int first, second;
  NAPI_CALL(env, napi_wrap(env, args[0], &first, NULL, NULL, NULL));
  napi_status status = napi_wrap(env, args[0], &second, NULL, NULL, NULL);
  napi_value result;
  NAPI_CALL(env, napi_create_int32(env, (int32_t)status, &result));
  return result;
}

napi_value register_classes(napi_env env, napi_value exports) {
  napi_value unit, version, ctor;
  NAPI_CALL(env, napi_create_string_utf8(env, "count", NAPI_AUTO_LENGTH, &unit));
  NAPI_CALL(env, napi_create_int32(env, 1, &version));
  napi_property_descriptor props[] = {
      {"increment", NULL, counter_increment, NULL, NULL, NULL, napi_default_method, NULL},
      {"value", NULL, NULL, counter_get, counter_set, NULL, napi_default_jsproperty, NULL},
      {"unit", NULL, NULL, NULL, NULL, unit, napi_default, NULL},
      {"kind", NULL, counter_kind, NULL, NULL, NULL, napi_static | napi_default_method, NULL},
      {"VERSION", NULL, NULL, NULL, NULL, version, napi_static | napi_enumerable, NULL},
  };
  NAPI_CALL(env, napi_define_class(env, "Counter", NAPI_AUTO_LENGTH, counter_new, NULL, 5,
                                   props, &ctor));
  NAPI_CALL(env, napi_set_named_property(env, exports, "Counter", ctor));
  NAPI_CALL(env, export_function(env, exports, "detach", detach));
  NAPI_CALL(env, export_function(env, exports, "wrapTwice", wrap_twice));
  return exports;
}

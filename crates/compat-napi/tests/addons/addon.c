/* Module lifecycle and instance data. Built with -DLEGACY_REGISTER it registers through the
 * deprecated `napi_module_register` static constructor instead of `napi_register_module_v1`. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "addon.h"

int counters[COUNTER_COUNT];

int velyr_test_counter(int which) { return counters[which]; }

void throw_last_error(napi_env env, const char* call) {
  const napi_extended_error_info* info = NULL;
  napi_get_last_error_info(env, &info);
  const char* message =
      (info != NULL && info->error_message != NULL) ? info->error_message : "no message";
  bool pending = false;
  napi_is_exception_pending(env, &pending);
  if (pending) return;
  char buffer[256];
  snprintf(buffer, sizeof buffer, "%s: %s", call, message);
  napi_throw_error(env, "ADDON_CALL_FAILED", buffer);
}

napi_status export_function(napi_env env, napi_value exports, const char* name,
                            napi_callback cb) {
  napi_value fn;
  napi_status status = napi_create_function(env, name, NAPI_AUTO_LENGTH, cb, NULL, &fn);
  if (status != napi_ok) return status;
  return napi_set_named_property(env, exports, name, fn);
}

napi_status set_named_int(napi_env env, napi_value object, const char* name, int32_t value) {
  napi_value number;
  napi_status status = napi_create_int32(env, value, &number);
  if (status != napi_ok) return status;
  return napi_set_named_property(env, object, name, number);
}

static void finalize_instance_data(napi_env env, void* data, void* hint) {
  (void)env;
  (void)hint;
  free(data);
  counters[COUNTER_INSTANCE_DATA_FINALIZED]++;
}

static napi_value get_instance_data(napi_env env, napi_callback_info info) {
  (void)info;
  int* data = NULL;
  NAPI_CALL(env, napi_get_instance_data(env, (void**)&data));
  napi_value result;
  NAPI_CALL(env, napi_create_int32(env, data == NULL ? -1 : *data, &result));
  return result;
}

static napi_value get_version(napi_env env, napi_callback_info info) {
  (void)info;
  uint32_t version = 0;
  NAPI_CALL(env, napi_get_version(env, &version));
  napi_value result;
  NAPI_CALL(env, napi_create_uint32(env, version, &result));
  return result;
}

static napi_value init(napi_env env, napi_value exports) {
  int* data = malloc(sizeof *data);
  *data = 42;
  NAPI_CALL(env, napi_set_instance_data(env, data, finalize_instance_data, NULL));
  NAPI_CALL(env, export_function(env, exports, "getInstanceData", get_instance_data));
  NAPI_CALL(env, export_function(env, exports, "getVersion", get_version));
#ifdef LEGACY_REGISTER
  NAPI_CALL(env, set_named_int(env, exports, "legacy", 1));
#endif
  napi_value (*groups[])(napi_env, napi_value) = {
      register_values, register_strings, register_objects, register_functions,
      register_classes, register_errors, register_buffers, register_lifetime,
  };
  for (size_t i = 0; i < sizeof groups / sizeof groups[0]; i++) {
    if (groups[i](env, exports) == NULL) return NULL;
  }
  return exports;
}

#ifdef LEGACY_REGISTER
static napi_module legacy_module = {
    .nm_version = 1,
    .nm_flags = 0,
    .nm_filename = __FILE__,
    .nm_register_func = init,
    .nm_modname = "velyr_test_addon_legacy",
    .nm_priv = NULL,
    .reserved = {0},
};

__attribute__((constructor)) static void register_legacy(void) {
  napi_module_register(&legacy_module);
}
#else
NAPI_MODULE(velyr_test_addon, init)
#endif

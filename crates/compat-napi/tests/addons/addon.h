/* Shared helpers for the compat-napi test addon. One group of the Node-API per source file. */
#ifndef VELYR_TEST_ADDON_H
#define VELYR_TEST_ADDON_H

#include <node_api.h>

/* Throws the last Node-API error as a JS Error (unless one is pending) and returns NULL. */
#define NAPI_CALL(env, call)                                                   \
  do {                                                                         \
    if ((call) != napi_ok) {                                                   \
      throw_last_error((env), #call);                                          \
      return NULL;                                                             \
    }                                                                          \
  } while (0)

/* Declares `argc` and `args[n]` filled from the callback info. */
#define GET_ARGS(env, info, n)                                                 \
  size_t argc = (n);                                                           \
  napi_value args[(n)];                                                        \
  NAPI_CALL((env), napi_get_cb_info((env), (info), &argc, args, NULL, NULL))

void throw_last_error(napi_env env, const char* call);
napi_status export_function(napi_env env, napi_value exports, const char* name,
                            napi_callback cb);
napi_status set_named_int(napi_env env, napi_value object, const char* name,
                          int32_t value);

/* Counters read by the Rust tests through `velyr_test_counter`. */
enum counter {
  COUNTER_INSTANCE_DATA_FINALIZED,
  COUNTER_OBJECT_FINALIZED,
  COUNTER_WRAP_FINALIZED,
  COUNTER_EXTERNAL_FINALIZED,
  COUNTER_COUNT
};
extern int counters[COUNTER_COUNT];

napi_value register_values(napi_env env, napi_value exports);
napi_value register_strings(napi_env env, napi_value exports);
napi_value register_objects(napi_env env, napi_value exports);
napi_value register_functions(napi_env env, napi_value exports);
napi_value register_classes(napi_env env, napi_value exports);
napi_value register_errors(napi_env env, napi_value exports);
napi_value register_buffers(napi_env env, napi_value exports);
napi_value register_lifetime(napi_env env, napi_value exports);

#endif

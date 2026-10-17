/* Functions and callbacks. */
#include <stdlib.h>

#include "addon.h"

/* callWith(fn, a, b): fn.call(globalThis, a, b). */
static napi_value call_with(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 3);
  napi_value global, result;
  NAPI_CALL(env, napi_get_global(env, &global));
  NAPI_CALL(env, napi_call_function(env, global, args[0], 2, args + 1, &result));
  return result;
}

static napi_value adder(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value args[1];
  int32_t* base;
  NAPI_CALL(env, napi_get_cb_info(env, info, &argc, args, NULL, (void**)&base));
  int32_t value;
  NAPI_CALL(env, napi_get_value_int32(env, args[0], &value));
  napi_value result;
  NAPI_CALL(env, napi_create_int32(env, *base + value, &result));
  return result;
}

static int32_t adder_bases[8];

/* makeAdder(n): a native function whose `data` points at n. */
static napi_value make_adder(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  static size_t next = 0;
  int32_t* base = &adder_bases[next++ % 8];
  NAPI_CALL(env, napi_get_value_int32(env, args[0], base));
  napi_value fn;
  NAPI_CALL(env, napi_create_function(env, "adder", NAPI_AUTO_LENGTH, adder, base, &fn));
  return fn;
}

/* construct(ctor, arg): new ctor(arg). */
static napi_value construct(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 2);
  napi_value result;
  NAPI_CALL(env, napi_new_instance(env, args[0], 1, args + 1, &result));
  return result;
}

/* argInfo(...): {argc, padded (args[argc] is undefined), self (this)}. */
static napi_value arg_info(napi_env env, napi_callback_info info) {
  size_t argc = 4;
  napi_value args[4], self, report;
  NAPI_CALL(env, napi_get_cb_info(env, info, &argc, args, &self, NULL));
  NAPI_CALL(env, napi_create_object(env, &report));
  NAPI_CALL(env, set_named_int(env, report, "argc", (int32_t)argc));
  NAPI_CALL(env, napi_set_named_property(env, report, "self", self));
  if (argc < 4) {
    napi_valuetype type;
    NAPI_CALL(env, napi_typeof(env, args[argc], &type));
    NAPI_CALL(env, set_named_int(env, report, "paddedType", (int32_t)type));
  }
  return report;
}

/* newTarget(): whether the call was a construct call. */
static napi_value new_target(napi_env env, napi_callback_info info) {
  napi_value target, result;
  NAPI_CALL(env, napi_get_new_target(env, info, &target));
  NAPI_CALL(env, napi_get_boolean(env, target != NULL, &result));
  return result;
}

napi_value register_functions(napi_env env, napi_value exports) {
  NAPI_CALL(env, export_function(env, exports, "callWith", call_with));
  NAPI_CALL(env, export_function(env, exports, "makeAdder", make_adder));
  NAPI_CALL(env, export_function(env, exports, "construct", construct));
  NAPI_CALL(env, export_function(env, exports, "argInfo", arg_info));
  NAPI_CALL(env, export_function(env, exports, "newTarget", new_target));
  return exports;
}

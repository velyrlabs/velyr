/* Handle scopes, references, finalizers and externals. */
#include <stdlib.h>

#include "addon.h"

/* scopeLoop(n): creates n objects, each inside its own handle scope; returns n. */
static napi_value scope_loop(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  uint32_t count;
  NAPI_CALL(env, napi_get_value_uint32(env, args[0], &count));
  for (uint32_t i = 0; i < count; i++) {
    napi_handle_scope scope;
    napi_value object;
    NAPI_CALL(env, napi_open_handle_scope(env, &scope));
    NAPI_CALL(env, napi_create_object(env, &object));
    NAPI_CALL(env, napi_close_handle_scope(env, scope));
  }
  napi_value result;
  NAPI_CALL(env, napi_create_uint32(env, count, &result));
  return result;
}

/* escape(): {value: "escaped", second: status of a second escape, mismatch: status of closing
 * an outer scope first}. */
static napi_value escape(napi_env env, napi_callback_info info) {
  (void)info;
  napi_escapable_handle_scope scope;
  napi_handle_scope inner;
  napi_value text, escaped, again, report;
  NAPI_CALL(env, napi_open_escapable_handle_scope(env, &scope));
  NAPI_CALL(env, napi_create_string_utf8(env, "escaped", NAPI_AUTO_LENGTH, &text));
  NAPI_CALL(env, napi_escape_handle(env, scope, text, &escaped));
  napi_status second = napi_escape_handle(env, scope, text, &again);
  NAPI_CALL(env, napi_open_handle_scope(env, &inner));
  napi_status mismatch = napi_close_escapable_handle_scope(env, scope);
  NAPI_CALL(env, napi_close_handle_scope(env, inner));
  NAPI_CALL(env, napi_close_escapable_handle_scope(env, scope));
  NAPI_CALL(env, napi_create_object(env, &report));
  NAPI_CALL(env, napi_set_named_property(env, report, "value", escaped));
  NAPI_CALL(env, set_named_int(env, report, "second", (int32_t)second));
  NAPI_CALL(env, set_named_int(env, report, "mismatch", (int32_t)mismatch));
  return report;
}

/* references(obj): {counts: [after ref, after unref], same: value is obj}. */
static napi_value references(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  napi_ref ref;
  uint32_t after_ref, after_unref;
  napi_value value, report;
  bool same;
  NAPI_CALL(env, napi_create_reference(env, args[0], 1, &ref));
  NAPI_CALL(env, napi_reference_ref(env, ref, &after_ref));
  NAPI_CALL(env, napi_reference_unref(env, ref, &after_unref));
  NAPI_CALL(env, napi_get_reference_value(env, ref, &value));
  NAPI_CALL(env, napi_strict_equals(env, value, args[0], &same));
  NAPI_CALL(env, napi_delete_reference(env, ref));
  NAPI_CALL(env, napi_create_object(env, &report));
  NAPI_CALL(env, set_named_int(env, report, "afterRef", (int32_t)after_ref));
  NAPI_CALL(env, set_named_int(env, report, "afterUnref", (int32_t)after_unref));
  NAPI_CALL(env, set_named_int(env, report, "same", same));
  return report;
}

static napi_ref weak_ref = NULL;

/* holdWeakly(obj): keeps a weak (count 0) reference to obj. */
static napi_value hold_weakly(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  if (weak_ref != NULL) NAPI_CALL(env, napi_delete_reference(env, weak_ref));
  NAPI_CALL(env, napi_create_reference(env, args[0], 0, &weak_ref));
  return NULL;
}

/* weakTarget(): the weakly held object, or undefined once collected. */
static napi_value weak_target(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value value;
  NAPI_CALL(env, napi_get_reference_value(env, weak_ref, &value));
  if (value == NULL) NAPI_CALL(env, napi_get_undefined(env, &value));
  return value;
}

static void finalize_object(napi_env env, void* data, void* hint) {
  (void)env;
  (void)data;
  (void)hint;
  counters[COUNTER_OBJECT_FINALIZED]++;
}

static napi_value attach_finalizer(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  NAPI_CALL(env, napi_add_finalizer(env, args[0], NULL, finalize_object, NULL, NULL));
  return args[0];
}

/* counter(which): one of the `counters` slots. */
static napi_value counter(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  uint32_t which;
  NAPI_CALL(env, napi_get_value_uint32(env, args[0], &which));
  napi_value result;
  NAPI_CALL(env, napi_create_int32(env, which < COUNTER_COUNT ? counters[which] : -1, &result));
  return result;
}

static void finalize_external(napi_env env, void* data, void* hint) {
  (void)env;
  (void)hint;
  free(data);
  counters[COUNTER_EXTERNAL_FINALIZED]++;
}

static napi_value make_external(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  int32_t* data = malloc(sizeof *data);
  NAPI_CALL(env, napi_get_value_int32(env, args[0], data));
  napi_value result;
  NAPI_CALL(env, napi_create_external(env, data, finalize_external, NULL, &result));
  return result;
}

static napi_value external_value(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  int32_t* data;
  NAPI_CALL(env, napi_get_value_external(env, args[0], (void**)&data));
  napi_value result;
  NAPI_CALL(env, napi_create_int32(env, *data, &result));
  return result;
}

napi_value register_lifetime(napi_env env, napi_value exports) {
  weak_ref = NULL;
  NAPI_CALL(env, export_function(env, exports, "scopeLoop", scope_loop));
  NAPI_CALL(env, export_function(env, exports, "escape", escape));
  NAPI_CALL(env, export_function(env, exports, "references", references));
  NAPI_CALL(env, export_function(env, exports, "holdWeakly", hold_weakly));
  NAPI_CALL(env, export_function(env, exports, "weakTarget", weak_target));
  NAPI_CALL(env, export_function(env, exports, "attachFinalizer", attach_finalizer));
  NAPI_CALL(env, export_function(env, exports, "counter", counter));
  NAPI_CALL(env, export_function(env, exports, "makeExternal", make_external));
  NAPI_CALL(env, export_function(env, exports, "externalValue", external_value));
  return exports;
}

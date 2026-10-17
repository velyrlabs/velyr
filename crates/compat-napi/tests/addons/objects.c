/* Objects, properties and arrays. */
#include "addon.h"

static napi_value bool_value(napi_env env, bool flag) {
  napi_value result;
  NAPI_CALL(env, napi_get_boolean(env, flag, &result));
  return result;
}

/* objectOps(obj): mutates `obj` and reports what the property API saw. */
static napi_value object_ops(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  napi_value obj = args[0], key, value, report, names;
  NAPI_CALL(env, set_named_int(env, obj, "added", 1));
  NAPI_CALL(env, napi_create_string_utf8(env, "viaKey", NAPI_AUTO_LENGTH, &key));
  NAPI_CALL(env, napi_create_int32(env, 2, &value));
  NAPI_CALL(env, napi_set_property(env, obj, key, value));
  bool has_key, has_own_key, has_named, has_inherited, own_inherited, deleted;
  NAPI_CALL(env, napi_has_property(env, obj, key, &has_key));
  NAPI_CALL(env, napi_has_own_property(env, obj, key, &has_own_key));
  NAPI_CALL(env, napi_has_named_property(env, obj, "added", &has_named));
  NAPI_CALL(env, napi_create_string_utf8(env, "toString", NAPI_AUTO_LENGTH, &key));
  NAPI_CALL(env, napi_has_property(env, obj, key, &has_inherited));
  NAPI_CALL(env, napi_has_own_property(env, obj, key, &own_inherited));
  NAPI_CALL(env, napi_create_string_utf8(env, "removeMe", NAPI_AUTO_LENGTH, &key));
  NAPI_CALL(env, napi_delete_property(env, obj, key, &deleted));

  NAPI_CALL(env, napi_create_object(env, &report));
  NAPI_CALL(env, napi_set_named_property(env, report, "hasKey", bool_value(env, has_key)));
  NAPI_CALL(env, napi_set_named_property(env, report, "hasOwnKey", bool_value(env, has_own_key)));
  NAPI_CALL(env, napi_set_named_property(env, report, "hasNamed", bool_value(env, has_named)));
  NAPI_CALL(env, napi_set_named_property(env, report, "hasInherited", bool_value(env, has_inherited)));
  NAPI_CALL(env, napi_set_named_property(env, report, "ownInherited", bool_value(env, own_inherited)));
  NAPI_CALL(env, napi_set_named_property(env, report, "deleted", bool_value(env, deleted)));
  NAPI_CALL(env, napi_get_named_property(env, obj, "existing", &value));
  NAPI_CALL(env, napi_set_named_property(env, report, "existing", value));
  NAPI_CALL(env, napi_create_string_utf8(env, "added", NAPI_AUTO_LENGTH, &key));
  NAPI_CALL(env, napi_get_property(env, obj, key, &value));
  NAPI_CALL(env, napi_set_named_property(env, report, "added", value));
  NAPI_CALL(env, napi_get_property_names(env, obj, &names));
  NAPI_CALL(env, napi_set_named_property(env, report, "names", names));
  return report;
}

static napi_value prop_method(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value result;
  NAPI_CALL(env, napi_create_string_utf8(env, "method", NAPI_AUTO_LENGTH, &result));
  return result;
}

static int accessor_store = 10;

static napi_value prop_getter(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value result;
  NAPI_CALL(env, napi_create_int32(env, accessor_store, &result));
  return result;
}

static napi_value prop_setter(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  NAPI_CALL(env, napi_get_value_int32(env, args[0], &accessor_store));
  return NULL;
}

static napi_value define_props(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  napi_value constant;
  NAPI_CALL(env, napi_create_int32(env, 7, &constant));
  napi_property_descriptor props[] = {
      {"constant", NULL, NULL, NULL, NULL, constant, napi_enumerable, NULL},
      {"method", NULL, prop_method, NULL, NULL, NULL, napi_default_method, NULL},
      {"accessor", NULL, NULL, prop_getter, prop_setter, NULL, napi_default_jsproperty, NULL},
  };
  NAPI_CALL(env, napi_define_properties(env, args[0], 3, props));
  return args[0];
}

static napi_value get_proto(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  napi_value result;
  NAPI_CALL(env, napi_get_prototype(env, args[0], &result));
  return result;
}

static napi_value is_instance(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 2);
  bool result;
  NAPI_CALL(env, napi_instanceof(env, args[0], args[1], &result));
  return bool_value(env, result);
}

/* makeSquares(n): [0, 1, 4, ...] built element by element. */
static napi_value make_squares(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  uint32_t length;
  NAPI_CALL(env, napi_get_value_uint32(env, args[0], &length));
  napi_value array, item;
  NAPI_CALL(env, napi_create_array_with_length(env, length, &array));
  for (uint32_t i = 0; i < length; i++) {
    NAPI_CALL(env, napi_create_uint32(env, i * i, &item));
    NAPI_CALL(env, napi_set_element(env, array, i, item));
  }
  return array;
}

/* arrayInfo(arr): {isArray, length, first, hasLast, deletedFirst, hasFirstAfter}. */
static napi_value array_info(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  bool is_array, has_last = false, deleted = false, has_first_after = false;
  uint32_t length = 0;
  napi_value report, first;
  NAPI_CALL(env, napi_is_array(env, args[0], &is_array));
  NAPI_CALL(env, napi_create_object(env, &report));
  NAPI_CALL(env, napi_set_named_property(env, report, "isArray", bool_value(env, is_array)));
  if (!is_array) return report;
  NAPI_CALL(env, napi_get_array_length(env, args[0], &length));
  NAPI_CALL(env, napi_get_element(env, args[0], 0, &first));
  NAPI_CALL(env, napi_has_element(env, args[0], length - 1, &has_last));
  NAPI_CALL(env, napi_delete_element(env, args[0], 0, &deleted));
  NAPI_CALL(env, napi_has_element(env, args[0], 0, &has_first_after));
  NAPI_CALL(env, set_named_int(env, report, "length", (int32_t)length));
  NAPI_CALL(env, napi_set_named_property(env, report, "first", first));
  NAPI_CALL(env, napi_set_named_property(env, report, "hasLast", bool_value(env, has_last)));
  NAPI_CALL(env, napi_set_named_property(env, report, "deletedFirst", bool_value(env, deleted)));
  NAPI_CALL(env, napi_set_named_property(env, report, "hasFirstAfter",
                                         bool_value(env, has_first_after)));
  return report;
}

napi_value register_objects(napi_env env, napi_value exports) {
  NAPI_CALL(env, export_function(env, exports, "objectOps", object_ops));
  NAPI_CALL(env, export_function(env, exports, "defineProps", define_props));
  NAPI_CALL(env, export_function(env, exports, "getProto", get_proto));
  NAPI_CALL(env, export_function(env, exports, "isInstance", is_instance));
  NAPI_CALL(env, export_function(env, exports, "makeSquares", make_squares));
  NAPI_CALL(env, export_function(env, exports, "arrayInfo", array_info));
  return exports;
}

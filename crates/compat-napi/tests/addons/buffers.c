/* ArrayBuffer, TypedArray and Buffer. */
#include <stdlib.h>
#include <string.h>

#include "addon.h"

/* makeArrayBuffer(n): bytes 0..n-1. */
static napi_value make_array_buffer(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  uint32_t length;
  NAPI_CALL(env, napi_get_value_uint32(env, args[0], &length));
  uint8_t* data;
  napi_value result;
  NAPI_CALL(env, napi_create_arraybuffer(env, length, (void**)&data, &result));
  for (uint32_t i = 0; i < length; i++) data[i] = (uint8_t)i;
  return result;
}

/* arrayBufferSum(ab): sum of its bytes, or -1 when not an ArrayBuffer. */
static napi_value array_buffer_sum(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  bool is_array_buffer;
  NAPI_CALL(env, napi_is_arraybuffer(env, args[0], &is_array_buffer));
  int32_t sum = -1;
  if (is_array_buffer) {
    uint8_t* data;
    size_t length;
    NAPI_CALL(env, napi_get_arraybuffer_info(env, args[0], (void**)&data, &length));
    sum = 0;
    for (size_t i = 0; i < length; i++) sum += data[i];
  }
  napi_value result;
  NAPI_CALL(env, napi_create_int32(env, sum, &result));
  return result;
}

/* makeTypedArray(type, length, byteOffset): a view over a fresh 16-byte ArrayBuffer. */
static napi_value make_typed_array(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 3);
  uint32_t type, length, offset;
  NAPI_CALL(env, napi_get_value_uint32(env, args[0], &type));
  NAPI_CALL(env, napi_get_value_uint32(env, args[1], &length));
  NAPI_CALL(env, napi_get_value_uint32(env, args[2], &offset));
  void* data;
  napi_value buffer, result;
  NAPI_CALL(env, napi_create_arraybuffer(env, 16, &data, &buffer));
  memset(data, 1, 16);
  NAPI_CALL(env, napi_create_typedarray(env, (napi_typedarray_type)type, length, buffer, offset,
                                        &result));
  return result;
}

/* typedArrayInfo(ta): {isTypedArray, type, length, byteOffset, firstByte, buffer}. */
static napi_value typed_array_info(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  bool is_typed_array;
  napi_value report, buffer;
  NAPI_CALL(env, napi_is_typedarray(env, args[0], &is_typed_array));
  NAPI_CALL(env, napi_create_object(env, &report));
  NAPI_CALL(env, set_named_int(env, report, "isTypedArray", is_typed_array));
  if (!is_typed_array) return report;
  napi_typedarray_type type;
  size_t length, offset;
  uint8_t* data;
  NAPI_CALL(env, napi_get_typedarray_info(env, args[0], &type, &length, (void**)&data, &buffer,
                                          &offset));
  NAPI_CALL(env, set_named_int(env, report, "type", (int32_t)type));
  NAPI_CALL(env, set_named_int(env, report, "length", (int32_t)length));
  NAPI_CALL(env, set_named_int(env, report, "byteOffset", (int32_t)offset));
  NAPI_CALL(env, set_named_int(env, report, "firstByte", length > 0 ? data[0] : -1));
  NAPI_CALL(env, napi_set_named_property(env, report, "buffer", buffer));
  return report;
}

/* makeBuffer(text): Buffer copy of the UTF-8 text. */
static napi_value make_buffer(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  char text[64];
  size_t length;
  NAPI_CALL(env, napi_get_value_string_utf8(env, args[0], text, sizeof text, &length));
  napi_value result;
  NAPI_CALL(env, napi_create_buffer_copy(env, length, text, NULL, &result));
  return result;
}

/* makeFilledBuffer(n): Buffer of n bytes 'a', 'b', ... */
static napi_value make_filled_buffer(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  uint32_t length;
  NAPI_CALL(env, napi_get_value_uint32(env, args[0], &length));
  char* data;
  napi_value result;
  NAPI_CALL(env, napi_create_buffer(env, length, (void**)&data, &result));
  for (uint32_t i = 0; i < length; i++) data[i] = (char)('a' + i % 26);
  return result;
}

static void finalize_external(napi_env env, void* data, void* hint) {
  (void)env;
  (void)hint;
  free(data);
  counters[COUNTER_EXTERNAL_FINALIZED]++;
}

/* makeExternalBuffer(): Buffer over malloc'd "ext" released by a finalizer. */
static napi_value make_external_buffer(napi_env env, napi_callback_info info) {
  (void)info;
  char* data = malloc(3);
  memcpy(data, "ext", 3);
  napi_value result;
  NAPI_CALL(env, napi_create_external_buffer(env, 3, data, finalize_external, NULL, &result));
  return result;
}

/* bufferText(buf): {isBuffer, text} where text is the bytes as UTF-8. */
static napi_value buffer_text(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  bool is_buffer;
  napi_value report, text;
  NAPI_CALL(env, napi_is_buffer(env, args[0], &is_buffer));
  NAPI_CALL(env, napi_create_object(env, &report));
  NAPI_CALL(env, set_named_int(env, report, "isBuffer", is_buffer));
  if (!is_buffer) return report;
  char* data;
  size_t length;
  NAPI_CALL(env, napi_get_buffer_info(env, args[0], (void**)&data, &length));
  NAPI_CALL(env, napi_create_string_utf8(env, data, length, &text));
  NAPI_CALL(env, napi_set_named_property(env, report, "text", text));
  return report;
}

napi_value register_buffers(napi_env env, napi_value exports) {
  NAPI_CALL(env, export_function(env, exports, "makeArrayBuffer", make_array_buffer));
  NAPI_CALL(env, export_function(env, exports, "arrayBufferSum", array_buffer_sum));
  NAPI_CALL(env, export_function(env, exports, "makeTypedArray", make_typed_array));
  NAPI_CALL(env, export_function(env, exports, "typedArrayInfo", typed_array_info));
  NAPI_CALL(env, export_function(env, exports, "makeBuffer", make_buffer));
  NAPI_CALL(env, export_function(env, exports, "makeFilledBuffer", make_filled_buffer));
  NAPI_CALL(env, export_function(env, exports, "makeExternalBuffer", make_external_buffer));
  NAPI_CALL(env, export_function(env, exports, "bufferText", buffer_text));
  return exports;
}

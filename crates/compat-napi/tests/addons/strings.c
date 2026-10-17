/* UTF-8, UTF-16 and Latin-1 strings. */
#include <stdlib.h>

#include "addon.h"

static napi_value echo_utf8(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  size_t length;
  NAPI_CALL(env, napi_get_value_string_utf8(env, args[0], NULL, 0, &length));
  char* buffer = malloc(length + 1);
  size_t copied;
  napi_status status = napi_get_value_string_utf8(env, args[0], buffer, length + 1, &copied);
  napi_value result = NULL;
  if (status == napi_ok) status = napi_create_string_utf8(env, buffer, copied, &result);
  free(buffer);
  NAPI_CALL(env, status);
  return result;
}

static napi_value echo_utf16(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  size_t length;
  NAPI_CALL(env, napi_get_value_string_utf16(env, args[0], NULL, 0, &length));
  char16_t* buffer = malloc((length + 1) * sizeof *buffer);
  size_t copied;
  napi_status status = napi_get_value_string_utf16(env, args[0], buffer, length + 1, &copied);
  napi_value result = NULL;
  if (status == napi_ok) status = napi_create_string_utf16(env, buffer, copied, &result);
  free(buffer);
  NAPI_CALL(env, status);
  return result;
}

static napi_value echo_latin1(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 1);
  size_t length;
  NAPI_CALL(env, napi_get_value_string_latin1(env, args[0], NULL, 0, &length));
  char* buffer = malloc(length + 1);
  size_t copied;
  napi_status status = napi_get_value_string_latin1(env, args[0], buffer, length + 1, &copied);
  napi_value result = NULL;
  if (status == napi_ok) status = napi_create_string_latin1(env, buffer, copied, &result);
  free(buffer);
  NAPI_CALL(env, status);
  return result;
}

/* utf8Prefix(text, capacity): reads into a `capacity`-byte buffer (NUL included). */
static napi_value utf8_prefix(napi_env env, napi_callback_info info) {
  GET_ARGS(env, info, 2);
  uint32_t capacity;
  NAPI_CALL(env, napi_get_value_uint32(env, args[1], &capacity));
  char buffer[64];
  size_t copied;
  NAPI_CALL(env, napi_get_value_string_utf8(env, args[0], buffer, capacity, &copied));
  napi_value result;
  NAPI_CALL(env, napi_create_string_utf8(env, buffer, NAPI_AUTO_LENGTH, &result));
  return result;
}

static napi_value fixed_strings(napi_env env, napi_callback_info info) {
  (void)info;
  static const char16_t utf16[] = {0x68, 0xD83D, 0xDE00, 0};
  napi_value result, item;
  NAPI_CALL(env, napi_create_array(env, &result));
  NAPI_CALL(env, napi_create_string_utf8(env, "caf\xC3\xA9 extra", 5, &item));
  NAPI_CALL(env, napi_set_element(env, result, 0, item));
  NAPI_CALL(env, napi_create_string_utf16(env, utf16, NAPI_AUTO_LENGTH, &item));
  NAPI_CALL(env, napi_set_element(env, result, 1, item));
  NAPI_CALL(env, napi_create_string_latin1(env, "caf\xE9", NAPI_AUTO_LENGTH, &item));
  NAPI_CALL(env, napi_set_element(env, result, 2, item));
  return result;
}

napi_value register_strings(napi_env env, napi_value exports) {
  NAPI_CALL(env, export_function(env, exports, "echoUtf8", echo_utf8));
  NAPI_CALL(env, export_function(env, exports, "echoUtf16", echo_utf16));
  NAPI_CALL(env, export_function(env, exports, "echoLatin1", echo_latin1));
  NAPI_CALL(env, export_function(env, exports, "utf8Prefix", utf8_prefix));
  NAPI_CALL(env, export_function(env, exports, "fixedStrings", fixed_strings));
  return exports;
}

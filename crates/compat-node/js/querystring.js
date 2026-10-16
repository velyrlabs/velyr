'use strict';
// node:querystring (the legacy parser). Written for this runtime; no Node source is copied.
//
// Known limits: `escape` and `unescape` lean on encodeURIComponent/decodeURIComponent, so the
// set of characters left unescaped is the encodeURIComponent one (the same set Node uses).
const errors = require('internal/errors');
const { Buffer } = require('buffer');

const DEFAULT_MAX_KEYS = 1000;

function escape(value) {
  if (typeof value !== 'string') {
    if (typeof value === 'object') value = String(value);
    else value += '';
  }
  try {
    return encodeURIComponent(value);
  } catch {
    throw errors.make(URIError, 'ERR_INVALID_URI', 'URI malformed');
  }
}

const hexValue = (code) => {
  if (code >= 48 && code <= 57) return code - 48;
  if (code >= 97 && code <= 102) return code - 87;
  if (code >= 65 && code <= 70) return code - 55;
  return -1;
};

function unescapeBuffer(value, decodeSpaces) {
  const input = Buffer.from(String(value));
  const out = Buffer.allocUnsafe(input.length);
  let length = 0;
  for (let i = 0; i < input.length; i++) {
    const byte = input[i];
    if (byte === 37 && i + 2 < input.length && hexValue(input[i + 1]) >= 0 && hexValue(input[i + 2]) >= 0) {
      out[length++] = (hexValue(input[i + 1]) << 4) | hexValue(input[i + 2]);
      i += 2;
    } else {
      out[length++] = decodeSpaces && byte === 43 ? 32 : byte;
    }
  }
  return out.subarray(0, length);
}

function unescape(value, decodeSpaces) {
  try {
    return decodeURIComponent(value);
  } catch {
    return unescapeBuffer(value, decodeSpaces).toString();
  }
}

function stringifyPrimitive(value) {
  if (typeof value === 'string') return value;
  if (typeof value === 'number' && Number.isFinite(value)) return String(value);
  if (typeof value === 'bigint') return String(value);
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  return '';
}

function stringify(object, separator = '&', equals = '=', options) {
  const encode = (options && typeof options.encodeURIComponent === 'function') ? options.encodeURIComponent : escape;
  if (object === null || typeof object !== 'object') return '';
  const pairs = [];
  for (const key of Object.keys(object)) {
    const value = object[key];
    const name = encode(stringifyPrimitive(key)) + equals;
    if (Array.isArray(value)) {
      for (const item of value) pairs.push(name + encode(stringifyPrimitive(item)));
    } else {
      pairs.push(name + encode(stringifyPrimitive(value)));
    }
  }
  return pairs.join(separator);
}

function parse(input, separator, equals, options) {
  const result = { __proto__: null };
  if (typeof input !== 'string' || input.length === 0) return result;
  const sep = separator ? String(separator) : '&';
  const eq = equals ? String(equals) : '=';
  let maxKeys = DEFAULT_MAX_KEYS;
  if (options && typeof options.maxKeys === 'number') maxKeys = options.maxKeys;
  const decode = (options && typeof options.decodeURIComponent === 'function') ? options.decodeURIComponent : unescape;
  const decodePart = (part) => {
    const spaced = part.includes('+') ? part.replaceAll('+', '%20') : part;
    try { return decode(spaced); } catch { return unescape(spaced, true); }
  };
  let segments = input.split(sep);
  if (maxKeys > 0 && segments.length > maxKeys) segments = segments.slice(0, maxKeys);
  for (const segment of segments) {
    if (segment.length === 0) continue;
    const at = segment.indexOf(eq);
    const key = decodePart(at >= 0 ? segment.slice(0, at) : segment);
    const value = at >= 0 ? decodePart(segment.slice(at + eq.length)) : '';
    if (!(key in result)) result[key] = value;
    else if (Array.isArray(result[key])) result[key].push(value);
    else result[key] = [result[key], value];
  }
  return result;
}

module.exports = {
  unescapeBuffer, unescape, escape, stringify, encode: stringify, parse, decode: parse,
};

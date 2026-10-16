'use strict';
// node:crypto random helpers and timingSafeEqual, over the CSPRNG op. Written for this runtime.
//
// Known limits: the callback forms generate synchronously and call back on the next tick.
// timingSafeEqual compares in aws-lc-rs (constant time in the contents, not in the lengths).
const errors = require('internal/errors');
const util = require('internal/crypto/util');

const MAX_SIZE = 2 ** 31 - 1;
const RANGE_BITS = 2 ** 48;

function randomBytesSync(size) {
  return util.run('random', util.text(size))[0];
}

function randomBytes(size, callback) {
  util.validateInteger(size, 'size', 0, MAX_SIZE);
  if (callback === undefined) return randomBytesSync(size);
  util.validateCallback(callback);
  const bytes = randomBytesSync(size);
  require('process').nextTick(callback, null, bytes);
  return undefined;
}

function region(buffer, offset, size) {
  const view = buffer instanceof ArrayBuffer ? new Uint8Array(buffer)
    : ArrayBuffer.isView(buffer) ? new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength) : undefined;
  if (view === undefined) throw errors.invalidArgType('buf', ['ArrayBuffer', 'Buffer', 'TypedArray', 'DataView'], buffer);
  util.validateInteger(offset, 'offset', 0, view.length);
  const length = size === undefined ? view.length - offset : size;
  util.validateInteger(length, 'size', 0, view.length - offset);
  return view.subarray(offset, offset + length);
}

function randomFillSync(buffer, offset = 0, size) {
  const target = region(buffer, offset, size);
  if (target.length > 0) target.set(randomBytesSync(target.length));
  return buffer;
}

function randomFill(buffer, offset, size, callback) {
  if (typeof offset === 'function') [callback, offset, size] = [offset, 0, undefined];
  else if (typeof size === 'function') [callback, size] = [size, undefined];
  util.validateCallback(callback);
  const filled = randomFillSync(buffer, offset, size);
  require('process').nextTick(callback, null, filled);
}

function checkSafeInteger(value, name) {
  if (!Number.isSafeInteger(value)) throw errors.invalidArgType(name, 'safe integer', value);
}

function randomInt(min, max, callback) {
  if (typeof max === 'undefined' || typeof max === 'function') [callback, max, min] = [max, min, 0];
  checkSafeInteger(min, 'min');
  checkSafeInteger(max, 'max');
  const span = max - min;
  if (span <= 0 || span >= RANGE_BITS) {
    throw errors.outOfRange('max', `greater than the value of "min" (${min}) and the range must be < 2^48`, max);
  }
  if (callback !== undefined) util.validateCallback(callback);
  // Rejection sampling over 48 random bits keeps the result uniform.
  const limit = RANGE_BITS - (RANGE_BITS % span);
  let value;
  do value = randomBytesSync(6).readUIntBE(0, 6); while (value >= limit);
  const result = min + (value % span);
  if (callback === undefined) return result;
  require('process').nextTick(callback, null, result);
  return undefined;
}

function randomUUID() {
  const bytes = randomBytesSync(16);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = bytes.toString('hex');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function timingSafeEqual(a, b) {
  const view = (value, name) => {
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    throw errors.invalidArgType(name, ['ArrayBuffer', 'Buffer', 'TypedArray', 'DataView'], value);
  };
  return util.run('timing_safe_equal', view(a, 'buf1'), view(b, 'buf2'))[0][0] === 1;
}

module.exports = { randomBytes, randomFill, randomFillSync, randomInt, randomUUID, timingSafeEqual };

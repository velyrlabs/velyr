'use strict';
// node:fs internals shared by the other fs files: argument coercion, the framed-result protocol
// of the Rust ops (see fs_ops.rs) and Node-style error translation. Written for this runtime.
const errors = require('internal/errors');
const ops = require('internal/ops');
const { Buffer } = require('buffer');

/** Builds the Error for an error frame: [code, errno, syscall, path, dest, message, cause]. */
function translate(parts) {
  const [code, errno, syscall, path, dest, message, cause] = parts.map((part) => part.toString());
  if (code === 'ERR_ACCESS_DENIED') {
    const error = errors.make(Error, code, message);
    const inner = errors.make(Error, cause || 'VELYR_PERMISSION_DENIED', 'permission denied');
    Object.defineProperty(error, 'cause', { value: inner, writable: true, configurable: true });
    return error;
  }
  if (code.startsWith('ERR_')) {
    const Base = code === 'ERR_INVALID_ARG_VALUE' ? TypeError : Error;
    const error = errors.make(Base, code, message);
    error.errno = -Math.abs(Number(errno));
    error.syscall = syscall;
    if (path) error.path = path;
    return error;
  }
  let text = `${code}: ${message}, ${syscall}`;
  if (path) text += ` '${path}'`;
  if (dest) text += ` -> '${dest}'`;
  const error = errors.make(Error, code, text);
  error.errno = -Math.abs(Number(errno));
  error.syscall = syscall;
  if (path) error.path = path;
  if (dest) error.dest = dest;
  return error;
}

function unframe(buffer) {
  const bytes = new Uint8Array(buffer);
  const parts = [];
  for (let at = 1; at < bytes.length;) {
    const size = ((bytes[at] << 24) | (bytes[at + 1] << 16) | (bytes[at + 2] << 8) | bytes[at + 3]) >>> 0;
    parts.push(Buffer.from(buffer, at + 4, size));
    at += 4 + size;
  }
  if (bytes[0] === 1) throw translate(parts);
  return parts;
}

const bytes = (value) => (value instanceof Uint8Array ? value : Buffer.from(String(value)));

/** Runs a Rust op on the isolate thread; returns the list of result Buffers or throws. */
function run(name, ...args) {
  return unframe(ops.fsSync(name, ...args.map(bytes)));
}

/** Same op with the I/O on the blocking pool; resolves with the list of Buffers. */
function runAsync(name, ...args) {
  return ops.fsAsync(name, ...args.map(bytes)).then(unframe);
}

/** A path argument as text: string, Buffer/Uint8Array or `file:` URL. */
function toPath(value, name = 'path') {
  if (value !== null && typeof value === 'object' && value.protocol === 'file:' && typeof value.href === 'string') {
    return require('url').fileURLToPath(value);
  }
  const text = typeof value === 'string' ? value : value instanceof Uint8Array ? Buffer.from(value).toString() : undefined;
  if (text === undefined) throw errors.invalidArgType(name, ['string', 'Buffer', 'URL'], value);
  if (text.includes('\0')) throw errors.invalidArgValue(name, value, 'must be a string, Uint8Array, or URL without null bytes');
  return text;
}

function toFd(value, name = 'fd') {
  if (typeof value !== 'number') throw errors.invalidArgType(name, 'number', value);
  if (!Number.isInteger(value) || value < -(2 ** 31) || value > 2 ** 31 - 1) {
    throw errors.outOfRange(name, '>= 0 && <= 2147483647', value);
  }
  return value;
}

/** `options` may be an encoding string or an object; returns a fresh object with defaults applied. */
function getOptions(options, defaults = {}) {
  if (options === undefined || options === null || typeof options === 'function') return { ...defaults };
  if (typeof options === 'string') return { ...defaults, encoding: options };
  if (typeof options !== 'object') throw errors.invalidArgType('options', ['string', 'Object'], options);
  const merged = { ...defaults, ...options };
  if (typeof merged.encoding === 'string' && merged.encoding !== 'buffer' && !Buffer.isEncoding(merged.encoding)) {
    throw errors.invalidArgValue('encoding', merged.encoding, 'is invalid encoding');
  }
  return merged;
}

/** Bytes for a write: strings are encoded, views are used as they are. */
function toData(data, encoding) {
  if (typeof data === 'string') return Buffer.from(data, encoding || 'utf8');
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  throw errors.invalidArgType('data', ['string', 'Buffer', 'TypedArray', 'DataView'], data);
}

/** Octal strings and numbers; anything else is ERR_INVALID_ARG_TYPE. */
function toMode(value, fallback) {
  if (value === undefined || value === null) return fallback;
  if (typeof value === 'string' && /^[0-7]+$/.test(value)) return parseInt(value, 8);
  if (typeof value === 'number' && Number.isInteger(value) && value >= 0) return value;
  throw errors.invalidArgType('mode', ['string', 'integer'], value);
}

function toMillis(value, name) {
  if (value instanceof Date) return value.getTime();
  if (typeof value === 'string' && value.trim() !== '' && Number.isFinite(Number(value))) return Number(value) * 1000;
  if (typeof value === 'number' && Number.isFinite(value)) return value * 1000;
  throw errors.invalidArgType(name, ['number', 'string', 'Date'], value);
}

/** Delivers a promise's outcome to a Node-style callback, outside the promise chain. */
function settle(promise, callback, extra) {
  const { nextTick } = require('process');
  promise.then((value) => nextTick(callback, null, value, extra), (error) => nextTick(callback, error));
}

module.exports = { run, runAsync, toPath, toFd, getOptions, toData, toMode, toMillis, settle, bytes };

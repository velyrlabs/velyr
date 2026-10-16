'use strict';
// node:crypto internals shared by the other crypto files: byte coercion, the framed-result
// protocol of the Rust ops (see crypto_ops.rs) and Node-style error translation.
const errors = require('internal/errors');
const ops = require('internal/ops');
const { Buffer } = require('buffer');

const HASH_NAMES = ['md5', 'sha1', 'sha224', 'sha256', 'sha3-256', 'sha3-384', 'sha3-512', 'sha384', 'sha512', 'sha512-256'];
const RANGE_ERRORS = new Set(['ERR_OUT_OF_RANGE', 'ERR_CRYPTO_INVALID_KEYLEN', 'ERR_CRYPTO_INVALID_SCRYPT_PARAMS', 'ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH']);
const TYPE_ERRORS = new Set(['ERR_CRYPTO_INVALID_IV', 'ERR_CRYPTO_INVALID_DIGEST']);

/** Canonical lowercase digest name (`RSA-SHA256`, `sha-256`, `SHA2-256` all become `sha256`), or undefined. */
function normalizeHash(name) {
  if (typeof name !== 'string') return undefined;
  const normalized = name.toLowerCase().replace(/^rsa-/, '').replace(/withrsaencryption$/, '')
    .replace(/^sha-?(\d)/, 'sha$1').replace(/^sha2-/, 'sha').replace('/', '-');
  return HASH_NAMES.includes(normalized) ? normalized : undefined;
}

function translate(message) {
  const match = /^(ERR_[A-Z0-9_]+)\|([\s\S]*)$/.exec(message);
  if (!match) return new Error(message);
  const Base = RANGE_ERRORS.has(match[1]) ? RangeError : TYPE_ERRORS.has(match[1]) ? TypeError : Error;
  return errors.make(Base, match[1], match[2]);
}

function unframe(buffer) {
  const bytes = new Uint8Array(buffer);
  const parts = [];
  for (let at = 1; at < bytes.length;) {
    const size = ((bytes[at] << 24) | (bytes[at + 1] << 16) | (bytes[at + 2] << 8) | bytes[at + 3]) >>> 0;
    parts.push(Buffer.from(buffer, at + 4, size));
    at += 4 + size;
  }
  if (bytes[0] === 1) throw translate(parts[0].toString());
  return parts;
}

const text = (value) => Buffer.from(String(value));

/** Runs a Rust op synchronously; parts are Buffers/Uint8Arrays, the result is a list of Buffers. */
function run(name, ...parts) {
  return unframe(ops.cryptoSync(name, ...parts));
}

/** Same op on the blocking pool; resolves with the list of Buffers. */
function runAsync(name, ...parts) {
  return ops.cryptoAsync(name, ...parts).then(unframe);
}

// Frees native contexts (hash/hmac/cipher state) that JS abandons without finishing them.
const registry = typeof FinalizationRegistry === 'function'
  ? new FinalizationRegistry((id) => { try { run('ctx_free', text(id)); } catch { /* already freed */ } })
  : undefined;
const track = (owner, id) => { if (registry) registry.register(owner, id, owner); };
const untrack = (owner) => { if (registry) registry.unregister(owner); };

/** Delivers a promise's outcome to a Node-style callback, outside the promise chain. */
function settle(promise, callback) {
  const process = require('process');
  promise.then((value) => process.nextTick(callback, null, value), (error) => process.nextTick(callback, error));
}

function toBytes(value, name, encoding) {
  if (typeof value === 'string') return Buffer.from(value, encoding);
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  if (value && value.type === 'secret' && typeof value.export === 'function') return value.export();
  throw errors.invalidArgType(name, ['string', 'ArrayBuffer', 'Buffer', 'TypedArray', 'DataView'], value);
}

function encodeOutput(buffer, encoding) {
  return encoding === undefined || encoding === 'buffer' ? buffer : buffer.toString(encoding);
}

function validateInteger(value, name, min, max) {
  if (typeof value !== 'number') throw errors.invalidArgType(name, 'number', value);
  if (!Number.isInteger(value) || value < min || value > max) {
    throw errors.outOfRange(name, `>= ${min} && <= ${max}`, value);
  }
}

function validateCallback(callback) {
  errors.validateFunction(callback, 'callback');
}

module.exports = { HASH_NAMES, normalizeHash, run, runAsync, settle, track, untrack, toBytes, text, encodeOutput, validateInteger, validateCallback };

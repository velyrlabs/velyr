'use strict';
// node:crypto key derivation (pbkdf2, hkdf, scrypt). The async forms run on the blocking pool.
//
// Known limits: pbkdf2 and hkdf accept sha1, sha256, sha384 and sha512 only.
const errors = require('internal/errors');
const util = require('internal/crypto/util');

const MAX_INT32 = 2 ** 31 - 1;
const DIGEST_SIZE = { sha1: 20, sha256: 32, sha384: 48, sha512: 64 };

function kdfDigest(digest, label) {
  errors.validateString(digest, label);
  const name = util.normalizeHash(digest);
  if (!(name in DIGEST_SIZE)) throw errors.make(TypeError, 'ERR_CRYPTO_INVALID_DIGEST', `Invalid digest: ${digest}`);
  return name;
}

function pbkdf2Args(password, salt, iterations, keylen, digest) {
  const passwordBytes = util.toBytes(password, 'password');
  const saltBytes = util.toBytes(salt, 'salt');
  util.validateInteger(iterations, 'iterations', 1, MAX_INT32);
  util.validateInteger(keylen, 'keylen', 0, MAX_INT32);
  return ['pbkdf2', util.text(kdfDigest(digest, 'digest')), passwordBytes, saltBytes, util.text(iterations), util.text(keylen)];
}

function pbkdf2Sync(password, salt, iterations, keylen, digest) {
  return util.run(...pbkdf2Args(password, salt, iterations, keylen, digest))[0];
}

function pbkdf2(password, salt, iterations, keylen, digest, callback) {
  const args = pbkdf2Args(password, salt, iterations, keylen, digest);
  util.validateCallback(callback);
  util.settle(util.runAsync(...args).then((parts) => parts[0]), callback);
}

function hkdfArgs(digest, key, salt, info, keylen) {
  const name = kdfDigest(digest, 'digest');
  const parts = [util.toBytes(key, 'key'), util.toBytes(salt, 'salt'), util.toBytes(info, 'info')];
  util.validateInteger(keylen, 'length', 0, MAX_INT32);
  if (keylen > 255 * DIGEST_SIZE[name]) throw errors.make(RangeError, 'ERR_CRYPTO_INVALID_KEYLEN', 'Invalid key length');
  return ['hkdf', util.text(name), ...parts, util.text(keylen)];
}

const toArrayBuffer = (buffer) => buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength);

function hkdfSync(digest, key, salt, info, keylen) {
  return toArrayBuffer(util.run(...hkdfArgs(digest, key, salt, info, keylen))[0]);
}

function hkdf(digest, key, salt, info, keylen, callback) {
  const args = hkdfArgs(digest, key, salt, info, keylen);
  util.validateCallback(callback);
  util.settle(util.runAsync(...args).then((parts) => toArrayBuffer(parts[0])), callback);
}

const SCRYPT_DEFAULTS = { N: 16384, r: 8, p: 1, maxmem: 32 * 1024 * 1024 };
const SCRYPT_ALIASES = { N: 'cost', r: 'blockSize', p: 'parallelization' };

// Node accepts N/r/p or cost/blockSize/parallelization; out-of-range values are rejected in Rust.
function scryptArgs(password, salt, keylen, options) {
  const passwordBytes = util.toBytes(password, 'password');
  const saltBytes = util.toBytes(salt, 'salt');
  util.validateInteger(keylen, 'keylen', 0, MAX_INT32);
  if (options !== undefined) errors.validateObject(options, 'options');
  const params = {};
  for (const [key, value] of Object.entries(SCRYPT_DEFAULTS)) {
    const given = options?.[key] ?? options?.[SCRYPT_ALIASES[key]];
    params[key] = given === undefined ? value : given;
    if (!Number.isSafeInteger(params[key]) || params[key] < 0) {
      throw errors.make(RangeError, 'ERR_CRYPTO_INVALID_SCRYPT_PARAMS', `Invalid scrypt param: ${key}`);
    }
  }
  const { N, r, p, maxmem } = params;
  return ['scrypt', passwordBytes, saltBytes, util.text(N), util.text(r), util.text(p), util.text(maxmem), util.text(keylen)];
}

function scryptSync(password, salt, keylen, options) {
  return util.run(...scryptArgs(password, salt, keylen, options))[0];
}

function scrypt(password, salt, keylen, options, callback) {
  if (typeof options === 'function') [options, callback] = [undefined, options];
  const args = scryptArgs(password, salt, keylen, options);
  util.validateCallback(callback);
  util.settle(util.runAsync(...args).then((parts) => parts[0]), callback);
}

module.exports = { pbkdf2, pbkdf2Sync, hkdf, hkdfSync, scrypt, scryptSync };

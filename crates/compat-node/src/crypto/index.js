'use strict';
// node:crypto entry point. Written for this runtime; no Node source is copied.
//
// Built on aws-lc-rs through the ops in crypto_ops.rs. `webcrypto`, `subtle` and
// `getRandomValues` are the isolate's WebCrypto (`globalThis.crypto`), read when accessed, so
// they need the web-crypto module to be installed. See the notes at the top of each
// src/crypto/*.js for the known limits.
const hash = require('internal/crypto/hash');
const kdf = require('internal/crypto/kdf');
const random = require('internal/crypto/random');
const cipher = require('internal/crypto/cipher');
const keys = require('internal/crypto/keys');
const signing = require('internal/crypto/sign');

const constants = Object.freeze({
  RSA_PKCS1_PADDING: 1,
  RSA_NO_PADDING: 3,
  RSA_PKCS1_OAEP_PADDING: 4,
  RSA_PKCS1_PSS_PADDING: signing.RSA_PKCS1_PSS_PADDING,
  RSA_PSS_SALTLEN_DIGEST: -1,
  RSA_PSS_SALTLEN_MAX_SIGN: -2,
  RSA_PSS_SALTLEN_AUTO: -2,
});

const exported = {
  KeyObject: keys.KeyObject,
  Hash: hash.Hash,
  Hmac: hash.Hmac,
  Cipheriv: cipher.Cipheriv,
  Decipheriv: cipher.Decipheriv,
  Sign: signing.Sign,
  Verify: signing.Verify,
  constants,
  createCipheriv: cipher.createCipheriv,
  createDecipheriv: cipher.createDecipheriv,
  createHash: hash.createHash,
  createHmac: hash.createHmac,
  createPrivateKey: keys.createPrivateKey,
  createPublicKey: keys.createPublicKey,
  createSecretKey: keys.createSecretKey,
  createSign: signing.createSign,
  createVerify: signing.createVerify,
  generateKey: keys.generateKey,
  generateKeyPair: keys.generateKeyPair,
  generateKeyPairSync: keys.generateKeyPairSync,
  generateKeySync: keys.generateKeySync,
  getCiphers: cipher.getCiphers,
  getHashes: hash.getHashes,
  getRandomValues: (array) => globalThis.crypto.getRandomValues(array),
  hash: hash.hash,
  hkdf: kdf.hkdf,
  hkdfSync: kdf.hkdfSync,
  pbkdf2: kdf.pbkdf2,
  pbkdf2Sync: kdf.pbkdf2Sync,
  randomBytes: random.randomBytes,
  randomFill: random.randomFill,
  randomFillSync: random.randomFillSync,
  randomInt: random.randomInt,
  randomUUID: random.randomUUID,
  scrypt: kdf.scrypt,
  scryptSync: kdf.scryptSync,
  sign: signing.sign,
  timingSafeEqual: random.timingSafeEqual,
  verify: signing.verify,
};
for (const name of Object.keys(exported)) {
  Object.defineProperty(module.exports, name, { value: exported[name], writable: true, configurable: true, enumerable: true });
}
for (const name of ['subtle', 'webcrypto']) {
  Object.defineProperty(module.exports, name, {
    get: () => (name === 'subtle' ? globalThis.crypto?.subtle : globalThis.crypto),
    configurable: true,
    enumerable: true,
  });
}

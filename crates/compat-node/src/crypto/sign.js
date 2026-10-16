'use strict';
// node:crypto Sign/Verify and crypto.sign/crypto.verify. Written for this runtime.
//
// Known limits: RSA PKCS#1 v1.5 and PSS (salt length = digest length), ECDSA with DER signatures
// (ieee-p1363 not supported) and Ed25519. RSA signing needs sha256/384/512, ECDSA signing the
// curve's own digest (P-256 sha256, P-384 sha384, P-521 sha256/384/512). The message is buffered.
const errors = require('internal/errors');
const { Buffer } = require('buffer');
const util = require('internal/crypto/util');
const { prepareKey } = require('internal/crypto/keys');

const RSA_PKCS1_PSS_PADDING = 6;
const DIGEST_SIZE = { sha1: 20, sha224: 28, sha256: 32, sha384: 48, sha512: 64 };

function digestName(algorithm) {
  if (algorithm === null || algorithm === undefined) return '';
  errors.validateString(algorithm, 'algorithm');
  const name = util.normalizeHash(algorithm);
  if (name === undefined) throw new Error('Invalid digest: ' + algorithm);
  return name;
}

function paddingOf(key, hash) {
  if (key === null || typeof key !== 'object' || key.padding !== RSA_PKCS1_PSS_PADDING) return '';
  const salt = key.saltLength;
  if (salt !== undefined && salt !== -1 && salt !== DIGEST_SIZE[hash]) {
    throw errors.make(Error, 'ERR_CRYPTO_UNSUPPORTED_OPERATION', 'Only a PSS salt length equal to the digest length is supported');
  }
  if (key.dsaEncoding === 'ieee-p1363') {
    throw errors.make(Error, 'ERR_CRYPTO_UNSUPPORTED_OPERATION', 'dsaEncoding ieee-p1363 is not supported');
  }
  return 'pss';
}

function signArgs(algorithm, data, key) {
  const hash = digestName(algorithm);
  const state = prepareKey(key, 'private');
  if (hash === '' && state.algo !== 'ed25519') throw new Error('No default digest for this key type');
  return ['sign', util.text(hash), state.der, data, util.text(paddingOf(key, hash))];
}

function verifyArgs(algorithm, data, key, signature) {
  const hash = digestName(algorithm);
  const state = prepareKey(key, 'public');
  return ['verify', util.text(hash), state.der, data, util.toBytes(signature, 'signature'), util.text(paddingOf(key, hash))];
}

const isValid = (parts) => parts[0][0] === 1;

class Sign {
  constructor(algorithm) {
    errors.validateString(algorithm, 'algorithm');
    Object.defineProperty(this, '_state', { value: { hash: digestName(algorithm), chunks: [] } });
  }

  update(data, inputEncoding) {
    this._state.chunks.push(util.toBytes(data, 'data', inputEncoding));
    return this;
  }

  sign(privateKey, outputEncoding) {
    const message = Buffer.concat(this._state.chunks);
    return util.encodeOutput(util.run(...signArgs(this._state.hash, message, privateKey))[0], outputEncoding);
  }
}

class Verify {
  constructor(algorithm) {
    errors.validateString(algorithm, 'algorithm');
    Object.defineProperty(this, '_state', { value: { hash: digestName(algorithm), chunks: [] } });
  }

  update(data, inputEncoding) {
    this._state.chunks.push(util.toBytes(data, 'data', inputEncoding));
    return this;
  }

  verify(publicKey, signature, signatureEncoding) {
    const bytes = typeof signature === 'string' ? Buffer.from(signature, signatureEncoding) : signature;
    return isValid(util.run(...verifyArgs(this._state.hash, Buffer.concat(this._state.chunks), publicKey, bytes)));
  }
}

function sign(algorithm, data, key, callback) {
  const args = signArgs(algorithm, util.toBytes(data, 'data'), key);
  if (callback === undefined) return util.run(...args)[0];
  util.validateCallback(callback);
  util.settle(util.runAsync(...args).then((parts) => parts[0]), callback);
  return undefined;
}

function verify(algorithm, data, key, signature, callback) {
  const args = verifyArgs(algorithm, util.toBytes(data, 'data'), key, signature);
  if (callback === undefined) return isValid(util.run(...args));
  util.validateCallback(callback);
  util.settle(util.runAsync(...args).then(isValid), callback);
  return undefined;
}

module.exports = { Sign, Verify, createSign: (algorithm) => new Sign(algorithm), createVerify: (algorithm) => new Verify(algorithm), sign, verify, RSA_PKCS1_PSS_PADDING };

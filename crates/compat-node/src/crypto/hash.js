'use strict';
// node:crypto Hash and Hmac. Written for this runtime; no Node source is copied.
//
// State is incremental: each object owns a native context (see crypto_stream.rs) that is freed
// by digest() or, if abandoned, by a finalizer. Known limits: Hash and Hmac are not streams; the
// `outputLength` option is ignored.
const errors = require('internal/errors');
const util = require('internal/crypto/util');

const finalized = () => errors.make(Error, 'ERR_CRYPTO_HASH_FINALIZED', 'Digest already called');

class Digest {
  constructor(algorithm, id) {
    Object.defineProperty(this, '_state', { value: { algorithm, id, done: false } });
    util.track(this, id);
  }

  update(data, inputEncoding) {
    const state = this._state;
    if (state.done) throw finalized();
    util.run('ctx_update', util.text(state.id), util.toBytes(data, 'data', inputEncoding));
    return this;
  }

  _finish(outputEncoding) {
    const state = this._state;
    if (state.done) throw finalized();
    state.done = true;
    util.untrack(this);
    return util.encodeOutput(util.run('ctx_final', util.text(state.id))[0], outputEncoding);
  }
}

class Hash extends Digest {
  constructor(algorithm, id = util.run('hash_new', util.text(algorithm))[0].toString()) {
    super(algorithm, id);
  }

  digest(outputEncoding) {
    return this._finish(outputEncoding);
  }

  copy() {
    const state = this._state;
    if (state.done) throw finalized();
    return new Hash(state.algorithm, util.run('ctx_copy', util.text(state.id))[0].toString());
  }
}

class Hmac extends Digest {
  constructor(algorithm, key) {
    super(algorithm, util.run('hmac_new', util.text(algorithm), key)[0].toString());
  }

  digest(outputEncoding) {
    return this._finish(outputEncoding);
  }
}

function createHash(algorithm) {
  errors.validateString(algorithm, 'algorithm');
  const name = util.normalizeHash(algorithm);
  if (name === undefined) throw new Error('Digest method not supported');
  return new Hash(name);
}

function createHmac(algorithm, key) {
  errors.validateString(algorithm, 'hmac');
  const name = util.normalizeHash(algorithm);
  if (name === undefined) throw errors.make(TypeError, 'ERR_CRYPTO_INVALID_DIGEST', `Invalid digest: ${algorithm}`);
  return new Hmac(name, util.toBytes(key, 'key'));
}

function hash(algorithm, data, outputEncoding = 'hex') {
  errors.validateString(algorithm, 'algorithm');
  if (typeof data !== 'string' && !ArrayBuffer.isView(data)) throw errors.invalidArgType('input', ['string', 'Buffer', 'TypedArray', 'DataView'], data);
  return createHash(algorithm).update(data).digest(outputEncoding);
}

const getHashes = () => [...util.HASH_NAMES];

module.exports = { Hash, Hmac, createHash, createHmac, hash, getHashes };

'use strict';
// node:crypto Cipheriv/Decipheriv: aes-{128,192,256}-{cbc,gcm} and chacha20-poly1305.
// Written for this runtime; no Node source is copied.
//
// Streaming: AES-CBC with padding (both directions) and AES-GCM encryption return output from
// update() through a native context (crypto_stream.rs). Known limits: GCM decryption, CBC with
// setAutoPadding(false) and chacha20-poly1305 buffer in update() and return everything from
// final() (GCM plaintext is never released before the tag is verified); AEAD needs a 12-byte IV
// and a 16-byte tag (authTagLength other than 16 is rejected); no aes-ecb/ctr, no streams.
const errors = require('internal/errors');
const { Buffer } = require('buffer');
const util = require('internal/crypto/util');

const SPECS = Object.create(null);
for (const bits of [128, 192, 256]) {
  SPECS[`aes-${bits}-cbc`] = { key: bits / 8, iv: 16, aead: false };
  SPECS[`aes-${bits}-gcm`] = { key: bits / 8, iv: 12, aead: true };
}
SPECS['chacha20-poly1305'] = { key: 32, iv: 12, aead: true };
const ALIASES = { aes128: 'aes-128-cbc', aes192: 'aes-192-cbc', aes256: 'aes-256-cbc',
  'id-aes128-gcm': 'aes-128-gcm', 'id-aes192-gcm': 'aes-192-gcm', 'id-aes256-gcm': 'aes-256-gcm' };
const TAG_LENGTH = 16;

const invalidState = (operation) => errors.make(Error, 'ERR_CRYPTO_INVALID_STATE', `Invalid state for operation ${operation}`);
const invalidTag = (length) => errors.make(TypeError, 'ERR_CRYPTO_INVALID_AUTH_TAG', `Invalid authentication tag length: ${length}`);

class CipherBase {
  constructor(decrypt, cipher, key, iv, options) {
    errors.validateString(cipher, 'cipher');
    const name = ALIASES[cipher.toLowerCase()] || cipher.toLowerCase();
    const spec = SPECS[name];
    if (spec === undefined) throw errors.make(Error, 'ERR_CRYPTO_UNKNOWN_CIPHER', 'Unknown cipher');
    const keyBytes = util.toBytes(key, 'key');
    const ivBytes = iv === null ? new Uint8Array(0) : util.toBytes(iv, 'iv');
    if (keyBytes.length !== spec.key) throw errors.make(RangeError, 'ERR_CRYPTO_INVALID_KEYLEN', 'Invalid key length');
    if (ivBytes.length !== spec.iv) throw errors.make(TypeError, 'ERR_CRYPTO_INVALID_IV', 'Invalid initialization vector');
    if (options && options.authTagLength !== undefined && options.authTagLength !== TAG_LENGTH) throw invalidTag(options.authTagLength);
    Object.defineProperty(this, '_state', {
      value: { name, spec, decrypt, key: Buffer.from(keyBytes), iv: Buffer.from(ivBytes), aad: Buffer.alloc(0),
        chunks: [], padding: true, tag: undefined, done: false, id: undefined },
    });
  }

  _streams() {
    const state = this._state;
    if (state.name.endsWith('-cbc')) return state.padding;
    return state.name.endsWith('-gcm') && !state.decrypt;
  }

  _context() {
    const state = this._state;
    if (state.id === undefined) {
      state.id = util.run('cipher_new', util.text(state.name), util.text(state.decrypt ? 'decrypt' : 'encrypt'),
        state.key, state.iv, state.aad)[0].toString();
      util.track(this, state.id);
    }
    return util.text(state.id);
  }

  update(data, inputEncoding, outputEncoding) {
    const state = this._state;
    if (state.done) throw invalidState('update');
    const bytes = util.toBytes(data, 'data', inputEncoding);
    if (!this._streams()) {
      state.chunks.push(bytes);
      return util.encodeOutput(Buffer.alloc(0), outputEncoding);
    }
    return util.encodeOutput(util.run('ctx_update', this._context(), bytes)[0], outputEncoding);
  }

  final(outputEncoding) {
    const state = this._state;
    if (state.done) throw invalidState('final');
    state.done = true;
    if (this._streams()) {
      try {
        const [output, tag] = util.run('ctx_final', this._context());
        if (state.spec.aead && !state.decrypt) state.tag = tag;
        return util.encodeOutput(output, outputEncoding);
      } finally {
        util.untrack(this);
      }
    }
    const [output, tag] = util.run('cipher', util.text(state.name), util.text(state.decrypt ? 'decrypt' : 'encrypt'),
      state.key, state.iv, state.aad, Buffer.concat(state.chunks), state.tag || Buffer.alloc(0), util.text(state.padding ? 1 : 0));
    if (state.spec.aead && !state.decrypt) state.tag = tag;
    return util.encodeOutput(output, outputEncoding);
  }

  setAutoPadding(autoPadding = true) {
    if (this._state.done || this._state.id !== undefined || this._state.chunks.length > 0) throw invalidState('setAutoPadding');
    this._state.padding = Boolean(autoPadding);
    return this;
  }

  setAAD(buffer, options) {
    const state = this._state;
    if (!state.spec.aead || state.done || state.id !== undefined) throw invalidState('setAAD');
    state.aad = Buffer.concat([state.aad, util.toBytes(buffer, 'buffer', options && options.encoding)]);
    return this;
  }
}

class Cipheriv extends CipherBase {
  constructor(cipher, key, iv, options) {
    super(false, cipher, key, iv, options);
  }

  getAuthTag() {
    const state = this._state;
    if (!state.done || !state.spec.aead) throw invalidState('getAuthTag');
    return state.tag;
  }
}

class Decipheriv extends CipherBase {
  constructor(cipher, key, iv, options) {
    super(true, cipher, key, iv, options);
  }

  setAuthTag(buffer, encoding) {
    const state = this._state;
    if (!state.spec.aead || state.done || state.tag !== undefined) throw invalidState('setAuthTag');
    const tag = util.toBytes(buffer, 'buffer', encoding);
    if (tag.length !== TAG_LENGTH) throw invalidTag(tag.length);
    state.tag = Buffer.from(tag);
    return this;
  }
}

const createCipheriv = (cipher, key, iv, options) => new Cipheriv(cipher, key, iv, options);
const createDecipheriv = (cipher, key, iv, options) => new Decipheriv(cipher, key, iv, options);
const getCiphers = () => Object.keys(SPECS).sort();

module.exports = { Cipheriv, Decipheriv, createCipheriv, createDecipheriv, getCiphers };

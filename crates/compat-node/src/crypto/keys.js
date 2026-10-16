'use strict';
// node:crypto KeyObject, createPublicKey/createPrivateKey/createSecretKey and key generation.
// Written for this runtime; no Node source is copied.
//
// Keys are held as canonical DER (PKCS#8 private, SPKI public) and parsed by aws-lc-rs.
// Known limits: rsa, ec (P-256, P-384, P-521) and ed25519 only; no passphrase-protected PEM
// (import or export), no certificates, RSA keys of 2048..8192 bits, RSA public exponent 65537
// for generation.
const errors = require('internal/errors');
const { Buffer } = require('buffer');
const util = require('internal/crypto/util');
const { randomBytes } = require('internal/crypto/random');

const kState = Symbol('velyr.crypto.key');
const CURVES = { 'p-256': 'P-256', prime256v1: 'P-256', secp256r1: 'P-256', 'p-384': 'P-384', secp384r1: 'P-384',
  'p-521': 'P-521', secp521r1: 'P-521' };
const PEM_LABELS = { spki: 'PUBLIC KEY', pkcs1public: 'RSA PUBLIC KEY', pkcs8: 'PRIVATE KEY', pkcs1private: 'RSA PRIVATE KEY',
  sec1: 'EC PRIVATE KEY' };
const RSA_FIELDS = ['n', 'e', 'd', 'p', 'q', 'dp', 'dq', 'qi'];
let allowConstruct = false;

class KeyObject {
  constructor(state) {
    if (!allowConstruct) throw errors.invalidArgType('type', 'string', undefined);
    Object.defineProperty(this, kState, { value: state });
  }

  get type() { return this[kState].type; }

  get asymmetricKeyType() { return this[kState].algo; }

  get asymmetricKeyDetails() {
    const { algo, details } = this[kState];
    if (algo === 'rsa') return { modulusLength: Number(details[0]), publicExponent: BigInt(details[1]) };
    if (algo === 'ec') return { namedCurve: details[0] };
    return algo === undefined ? undefined : {};
  }

  get symmetricKeySize() { return this[kState].bytes && this[kState].bytes.length; }

  export(options) { return exportKey(this[kState], options); }

  equals(other) {
    if (!(other instanceof KeyObject)) throw errors.invalidArgType('otherKeyObject', 'KeyObject', other);
    const [a, b] = [this[kState], other[kState]];
    if (a.type !== b.type) return false;
    return a.type === 'secret' ? Buffer.compare(a.bytes, b.bytes) === 0 : Buffer.compare(a.der, b.der) === 0;
  }
}
Object.defineProperty(KeyObject.prototype, Symbol.toStringTag, { value: 'KeyObject', configurable: true });

function wrap(state) {
  allowConstruct = true;
  try { return new KeyObject(state); } finally { allowConstruct = false; }
}

const stateFrom = ([algo, type, der, ...details]) => ({ type: type.toString(), algo: algo.toString(), der, details: details.map(String) });

function parseKey(data, format, type, want) {
  return stateFrom(util.run('key_parse', data, util.text(format), util.text(type), util.text(want)));
}

function derivePublic(state) {
  return parseKey(util.run('key_public', state.der)[0], 'der', 'spki', 'public');
}

function checkRole(state, want) {
  if (state.type === 'secret') {
    throw errors.make(TypeError, 'ERR_CRYPTO_INVALID_KEY_OBJECT_TYPE', `Invalid key object type secret, expected ${want}.`);
  }
  if (want === 'private' && state.type !== 'private') throw new Error('Invalid private key: the input is a public key');
  return want === 'public' && state.type === 'private' ? derivePublic(state) : state;
}

const fromBase64Url = (value) => Buffer.from(String(value), 'base64url');

function importJwk(jwk, want) {
  errors.validateObject(jwk, 'key');
  let kind;
  let parts;
  if (jwk.kty === 'RSA') {
    kind = 'rsa';
    parts = RSA_FIELDS.slice(0, jwk.d === undefined ? 2 : 8).map((field) => fromBase64Url(jwk[field]));
  } else if (jwk.kty === 'EC') {
    kind = 'ec';
    parts = [util.text(jwk.crv), fromBase64Url(jwk.x), fromBase64Url(jwk.y)];
    if (jwk.d !== undefined) parts.push(fromBase64Url(jwk.d));
  } else if (jwk.kty === 'OKP' && jwk.crv === 'Ed25519') {
    kind = 'ed25519';
    parts = [util.text(jwk.crv), fromBase64Url(jwk.x)];
    if (jwk.d !== undefined) parts.push(fromBase64Url(jwk.d));
  } else {
    throw errors.make(Error, 'ERR_CRYPTO_INVALID_JWK', 'Invalid JWK data');
  }
  return checkRole(stateFrom(util.run('key_jwk_import', util.text(kind), ...parts)), want);
}

/** Resolves any key input (KeyObject, PEM, `{ key, format, type }`, JWK) to key state of the `want` role. */
function prepareKey(input, want) {
  if (input instanceof KeyObject) return checkRole(input[kState], want);
  const isOptions = input !== null && typeof input === 'object' && !ArrayBuffer.isView(input)
    && !(input instanceof ArrayBuffer) && 'key' in input;
  const options = isOptions ? input : {};
  const source = isOptions ? input.key : input;
  if (source instanceof KeyObject) return checkRole(source[kState], want);
  if (options.format === 'jwk') return importJwk(source, want);
  const data = typeof source === 'string' ? Buffer.from(source, options.encoding) : util.toBytes(source, 'key');
  return checkRole(parseKey(data, options.format === 'der' ? 'der' : 'pem', options.type || '', want), want);
}

function armor(der, label) {
  const lines = der.toString('base64').match(/.{1,64}/g).join('\n');
  return `-----BEGIN ${label}-----\n${lines}\n-----END ${label}-----\n`;
}

function exportJwk(state) {
  const parts = util.run('key_jwk_export', state.der, util.text(state.type));
  const b64 = (part) => part.toString('base64url');
  if (state.algo === 'rsa') return { kty: 'RSA', ...Object.fromEntries(parts.map((part, i) => [RSA_FIELDS[i], b64(part)])) };
  if (state.algo === 'ec') {
    const [crv, x, y, d] = parts;
    return { kty: 'EC', crv: crv.toString(), x: b64(x), y: b64(y), ...(d ? { d: b64(d) } : {}) };
  }
  const [crv, x, d] = parts;
  return { kty: 'OKP', crv: crv.toString(), x: b64(x), ...(d ? { d: b64(d) } : {}) };
}

function exportKey(state, options = {}) {
  if (state.type === 'secret') {
    return options.format === 'jwk' ? { kty: 'oct', k: state.bytes.toString('base64url') } : Buffer.from(state.bytes);
  }
  errors.validateObject(options, 'options');
  if (options.format === 'jwk') return exportJwk(state);
  if (options.cipher !== undefined || options.passphrase !== undefined) {
    throw errors.make(Error, 'ERR_FEATURE_UNAVAILABLE_ON_PLATFORM', 'Encrypted key export is not supported');
  }
  if (options.format !== 'pem' && options.format !== 'der') throw errors.invalidArgValue('options.format', options.format);
  const allowed = state.type === 'public' ? ['spki', 'pkcs1'] : ['pkcs8', 'pkcs1', 'sec1'];
  if (!allowed.includes(options.type)) throw errors.invalidArgValue('options.type', options.type);
  const der = util.run('key_export', state.der, util.text(state.type), util.text(options.type))[0];
  if (options.format === 'der') return der;
  return armor(der, PEM_LABELS[options.type === 'pkcs1' ? `pkcs1${state.type}` : options.type]);
}

function createSecretKey(key, encoding) {
  return wrap({ type: 'secret', bytes: Buffer.from(util.toBytes(key, 'key', encoding)) });
}

const createPublicKey = (key) => wrap(prepareKey(key, 'public'));
const createPrivateKey = (key) => wrap(prepareKey(key, 'private'));

function generationArgs(type, options) {
  errors.validateString(type, 'type');
  if (type === 'ed25519') return ['ed25519', ''];
  errors.validateObject(options, 'options');
  if (type === 'rsa') {
    util.validateInteger(options.modulusLength, 'options.modulusLength', 512, 2 ** 31 - 1);
    if (options.publicExponent !== undefined && options.publicExponent !== 65537) {
      throw errors.make(Error, 'ERR_CRYPTO_UNSUPPORTED_OPERATION', 'Only the public exponent 65537 is supported');
    }
    return ['rsa', String(options.modulusLength)];
  }
  const curve = type === 'ec' && CURVES[String(options.namedCurve).toLowerCase()];
  if (!curve) throw errors.invalidArgValue(type === 'ec' ? 'options.namedCurve' : 'type', type === 'ec' ? options.namedCurve : type);
  return ['ec', curve];
}

function finishPair([privateDer, publicDer], options) {
  const encode = (key, encoding) => (encoding === undefined ? key : key.export(encoding));
  const publicKey = wrap(parseKey(publicDer, 'der', 'spki', 'public'));
  const privateKey = wrap(parseKey(privateDer, 'der', 'pkcs8', 'private'));
  return { publicKey: encode(publicKey, options && options.publicKeyEncoding), privateKey: encode(privateKey, options && options.privateKeyEncoding) };
}

function generateKeyPairSync(type, options) {
  const args = generationArgs(type, options);
  return finishPair(util.run('key_generate', ...args.map(util.text)), options);
}

function generateKeyPair(type, options, callback) {
  if (typeof options === 'function') [callback, options] = [options, undefined];
  util.validateCallback(callback);
  const args = generationArgs(type, options);
  const process = require('process');
  util.runAsync('key_generate', ...args.map(util.text)).then((parts) => {
    const { publicKey, privateKey } = finishPair(parts, options);
    process.nextTick(callback, null, publicKey, privateKey);
  }, (error) => process.nextTick(callback, error));
}

function generateKeySync(type, options) {
  errors.validateString(type, 'type');
  errors.validateObject(options, 'options');
  if (type === 'aes') {
    if (![128, 192, 256].includes(options.length)) throw errors.invalidArgValue('options.length', options.length);
  } else if (type === 'hmac') {
    util.validateInteger(options.length, 'options.length', 8, 2 ** 31 - 1);
  } else {
    throw errors.invalidArgValue('type', type, "must be one of: 'hmac', 'aes'");
  }
  return createSecretKey(randomBytes(Math.ceil(options.length / 8)));
}

function generateKey(type, options, callback) {
  util.validateCallback(callback);
  let key;
  try { key = generateKeySync(type, options); } catch (error) { require('process').nextTick(callback, error); return; }
  require('process').nextTick(callback, null, key);
}

module.exports = { KeyObject, createSecretKey, createPublicKey, createPrivateKey, generateKeyPair, generateKeyPairSync,
  generateKey, generateKeySync, prepareKey };

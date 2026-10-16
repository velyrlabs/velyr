
  // ---- CryptoKey and helpers shared by the algorithm families ---------------------------------------
  const USAGE_ORDER = ["encrypt", "decrypt", "sign", "verify", "deriveKey", "deriveBits", "wrapKey", "unwrapKey"];
  const keyRecords = new WeakMap(); // CryptoKey -> { type, algorithm, extractable, usages, material }
  const IMPL = new Map(); // canonical algorithm name -> family object (generate, importKey, exportKey, ...)
  let allowKeyConstruct = false;

  function keyRecord(key) {
    const record = keyRecords.get(key);
    if (record === undefined) throw new TypeError("Illegal invocation: receiver is not a CryptoKey");
    return record;
  }

  class CryptoKey {
    constructor() {
      if (!allowKeyConstruct) throw new TypeError("Illegal constructor");
    }
    get type() { return keyRecord(this).type; }
    get extractable() { return keyRecord(this).extractable; }
    get algorithm() { return keyRecord(this).algorithm; }
    get usages() { return [...keyRecord(this).usages]; }
  }
  Object.defineProperty(CryptoKey.prototype, Symbol.toStringTag, { value: "CryptoKey", configurable: true });

  function freezeAlgorithm(algorithm) {
    if (algorithm.hash !== undefined) Object.freeze(algorithm.hash);
    return Object.freeze(algorithm);
  }

  function createKey({ type, algorithm, extractable, usages, material }) {
    allowKeyConstruct = true;
    let key;
    try {
      key = new CryptoKey();
    } finally {
      allowKeyConstruct = false;
    }
    keyRecords.set(key, {
      type, algorithm: freezeAlgorithm(algorithm), extractable,
      usages: USAGE_ORDER.filter((usage) => usages.includes(usage)), material,
    });
    return key;
  }

  const materialOf = (key) => keyRecords.get(key).material;

  function defineAlgorithm(name, ops, family) {
    register(name, ops);
    IMPL.set(name, family);
  }

  function toUsages(list) {
    if (list === null || typeof list !== "object" || typeof list[Symbol.iterator] !== "function") {
      throw new TypeError("keyUsages must be a sequence of strings");
    }
    const usages = [];
    for (const usage of list) {
      if (!USAGE_ORDER.includes(usage)) {
        throw new TypeError(`The provided value '${usage}' is not a valid enum value of type KeyUsage`);
      }
      if (!usages.includes(usage)) usages.push(usage);
    }
    return usages;
  }

  function checkUsages(usages, allowed) {
    if (usages.some((usage) => !allowed.includes(usage))) {
      throw domError("SyntaxError", "Unsupported key usage for this algorithm");
    }
  }

  function requireUsages(usages) {
    if (usages.length === 0) throw domError("SyntaxError", "Usages cannot be empty when creating a key.");
  }

  function sameBytes(a, b) {
    return a.length === b.length && a.every((byte, i) => byte === b[i]);
  }

  function concatBytes(...parts) {
    const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
    let at = 0;
    for (const part of parts) {
      out.set(part, at);
      at += part.length;
    }
    return out;
  }

  const BASE64URL = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

  function base64urlEncode(bytes) {
    let out = "";
    for (let i = 0; i < bytes.length; i += 3) {
      const n = (bytes[i] << 16) | ((bytes[i + 1] ?? 0) << 8) | (bytes[i + 2] ?? 0);
      out += BASE64URL[(n >> 18) & 63] + BASE64URL[(n >> 12) & 63];
      if (i + 1 < bytes.length) out += BASE64URL[(n >> 6) & 63];
      if (i + 2 < bytes.length) out += BASE64URL[n & 63];
    }
    return out;
  }

  function base64urlDecode(text, what) {
    if (typeof text !== "string" || /[^A-Za-z0-9_-]/.test(text) || text.length % 4 === 1) {
      throw domError("DataError", `${what} is not valid base64url`);
    }
    const bytes = [];
    let bits = 0;
    let acc = 0;
    for (const char of text) {
      acc = (acc << 6) | BASE64URL.indexOf(char);
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        bytes.push((acc >> bits) & 255);
      }
    }
    return new Uint8Array(bytes);
  }

  function jwkObject(value) {
    if (value === null || typeof value !== "object") throw new TypeError("keyData must be a JSON Web Key object");
    return value;
  }

  // The checks every JWK import shares (RFC 7517 members against the WebCrypto import arguments).
  function checkJwk(jwk, { kty, use, usages, extractable }) {
    if (jwk.kty !== kty) throw domError("DataError", `JWK kty must be ${kty}`);
    if (usages.length > 0 && jwk.use !== undefined && jwk.use !== use) {
      throw domError("DataError", `JWK use must be ${use}`);
    }
    if (jwk.key_ops !== undefined) {
      const ops = jwk.key_ops;
      if (!Array.isArray(ops) || ops.some((op) => !USAGE_ORDER.includes(op)) || new Set(ops).size !== ops.length) {
        throw domError("DataError", "JWK key_ops is invalid");
      }
      if (usages.some((usage) => !ops.includes(usage))) {
        throw domError("DataError", "JWK key_ops does not permit the requested usages");
      }
    }
    if (jwk.ext === false && extractable) throw domError("DataError", "JWK ext is false but the key must be extractable");
  }

  const jwkBase = (key) => ({ key_ops: key.usages, ext: key.extractable });
  const unsupportedFormat = (format, name) =>
    domError("NotSupportedError", `Unsupported key format ${format} for ${name}`);

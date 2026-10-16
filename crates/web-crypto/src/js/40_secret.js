
  // ---- HMAC and AES (secret keys) -------------------------------------------------------------------
  const HASH_BITS = { "SHA-1": 512, "SHA-256": 512, "SHA-384": 1024, "SHA-512": 1024 };
  const HMAC_JWK_ALG = { "SHA-1": "HS1", "SHA-256": "HS256", "SHA-384": "HS384", "SHA-512": "HS512" };

  function hashName(hash) {
    if (hash === undefined) throw new TypeError("algorithm.hash is required");
    return normalizeAlgorithm(hash, "digest").name;
  }

  const hmacFamily = {
    async generate(alg, extractable, usages) {
      const hash = hashName(alg.hash);
      checkUsages(usages, ["sign", "verify"]);
      requireUsages(usages);
      const bits = alg.length === undefined ? HASH_BITS[hash] : Number(alg.length);
      if (!(bits > 0)) throw domError("OperationError", "HMAC key length must be greater than zero");
      if (bits % 8 !== 0) {
        throw domError("NotSupportedError", "HMAC key lengths that are not a multiple of 8 bits are not supported");
      }
      return hmacKey(randomBytes(bits / 8), hash, extractable, usages);
    },

    async importKey(format, data, alg, extractable, usages) {
      const hash = hashName(alg.hash);
      checkUsages(usages, ["sign", "verify"]);
      let bytes;
      if (format === "raw") {
        bytes = data;
      } else if (format === "jwk") {
        checkJwk(data, { kty: "oct", use: "sig", usages, extractable });
        bytes = base64urlDecode(data.k, "JWK k");
        if (data.alg !== undefined && data.alg !== HMAC_JWK_ALG[hash]) {
          throw domError("DataError", "JWK alg does not match the HMAC hash");
        }
      } else {
        throw unsupportedFormat(format, "HMAC");
      }
      requireUsages(usages);
      const bits = bytes.length * 8;
      if (bits === 0) throw domError("DataError", "Zero-length key is not supported");
      if (alg.length !== undefined) {
        if (alg.length > bits || alg.length <= bits - 8) throw domError("DataError", "Invalid key length");
        if (alg.length !== bits) throw domError("NotSupportedError", "Truncated HMAC key lengths are not supported");
      }
      return hmacKey(bytes, hash, extractable, usages);
    },

    async exportKey(format, key) {
      const bytes = materialOf(key);
      if (format === "raw") return bytes.slice().buffer;
      if (format === "jwk") {
        return { kty: "oct", k: base64urlEncode(bytes), alg: HMAC_JWK_ALG[key.algorithm.hash.name], ...jwkBase(key) };
      }
      throw unsupportedFormat(format, "HMAC");
    },

    getKeyLength(alg) {
      const bits = alg.length === undefined ? HASH_BITS[hashName(alg.hash)] : alg.length;
      if (!(bits > 0)) throw domError("OperationError", "HMAC key length must be greater than zero");
      return bits;
    },

    async sign(alg, key, data) {
      const [mac] = await call("hmac_sign", ascii(key.algorithm.hash.name), materialOf(key), data);
      return mac.buffer;
    },

    async verify(alg, key, signature, data) {
      const [ok] = await call("hmac_verify", ascii(key.algorithm.hash.name), materialOf(key), data, signature);
      return ok[0] === 1;
    },
  };

  function hmacKey(bytes, hash, extractable, usages) {
    const algorithm = { name: "HMAC", hash: { name: hash }, length: bytes.length * 8 };
    return createKey({ type: "secret", algorithm, extractable, usages, material: bytes });
  }

  defineAlgorithm("HMAC", ["generateKey", "importKey", "sign", "verify", "get key length"], hmacFamily);

  // AES: `crypt(direction, alg, key, data)` is the mode-specific part and resolves to a Uint8Array.
  const AES_USAGES = ["encrypt", "decrypt", "wrapKey", "unwrapKey"];

  function aesFamily(name, mode, crypt, allowed = AES_USAGES) {
    const jwkAlg = (bits) => `A${bits}${mode}`;
    const aesKey = (bytes, extractable, usages) =>
      createKey({
        type: "secret", algorithm: { name, length: bytes.length * 8 }, extractable, usages, material: bytes,
      });
    return {
      async generate(alg, extractable, usages) {
        checkUsages(usages, allowed);
        requireUsages(usages);
        if (![128, 192, 256].includes(alg.length)) throw domError("OperationError", "AES key length must be 128, 192 or 256");
        return aesKey(randomBytes(alg.length / 8), extractable, usages);
      },

      async importKey(format, data, alg, extractable, usages) {
        checkUsages(usages, allowed);
        let bytes;
        if (format === "raw") {
          bytes = data;
        } else if (format === "jwk") {
          checkJwk(data, { kty: "oct", use: "enc", usages, extractable });
          bytes = base64urlDecode(data.k, "JWK k");
          if (data.alg !== undefined && data.alg !== jwkAlg(bytes.length * 8)) {
            throw domError("DataError", "JWK alg does not match the AES key length and mode");
          }
        } else {
          throw unsupportedFormat(format, name);
        }
        requireUsages(usages);
        if (![16, 24, 32].includes(bytes.length)) throw domError("DataError", "Invalid AES key length");
        return aesKey(bytes, extractable, usages);
      },

      async exportKey(format, key) {
        const bytes = materialOf(key);
        if (format === "raw") return bytes.slice().buffer;
        if (format === "jwk") {
          return { kty: "oct", k: base64urlEncode(bytes), alg: jwkAlg(bytes.length * 8), ...jwkBase(key) };
        }
        throw unsupportedFormat(format, name);
      },

      getKeyLength(alg) {
        if (![128, 192, 256].includes(alg.length)) throw domError("OperationError", "AES key length must be 128, 192 or 256");
        return alg.length;
      },

      async encrypt(alg, key, data) {
        return (await crypt("encrypt", alg, key, data)).buffer;
      },

      async decrypt(alg, key, data) {
        return (await crypt("decrypt", alg, key, data)).buffer;
      },
    };
  }

  const AES_OPS = ["generateKey", "importKey", "encrypt", "decrypt", "wrapKey", "unwrapKey", "get key length"];

  defineAlgorithm("AES-GCM", AES_OPS, aesFamily("AES-GCM", "GCM", async (direction, alg, key, data) => {
    const iv = copyBytes(alg.iv, "iv");
    const aad = alg.additionalData === undefined ? new Uint8Array(0) : copyBytes(alg.additionalData, "additionalData");
    const tagLength = alg.tagLength === undefined ? 128 : Number(alg.tagLength);
    if (![32, 64, 96, 104, 112, 120, 128].includes(tagLength)) {
      throw domError("OperationError", "Invalid AES-GCM tag length");
    }
    const [out] = await call("aes_gcm", ascii(direction), materialOf(key), iv, aad, data, ascii(String(tagLength)));
    return out;
  }));

  defineAlgorithm("AES-CBC", AES_OPS, aesFamily("AES-CBC", "CBC", async (direction, alg, key, data) => {
    const [out] = await call("aes_cbc", ascii(direction), materialOf(key), copyBytes(alg.iv, "iv"), data);
    return out;
  }));

  defineAlgorithm("AES-CTR", AES_OPS, aesFamily("AES-CTR", "CTR", async (direction, alg, key, data) => {
    const counter = copyBytes(alg.counter, "counter");
    const [out] = await call("aes_ctr", ascii(direction), materialOf(key), counter, ascii(String(alg.length)), data);
    return out;
  }));

  // AES-KW (RFC 3394): wrapKey / unwrapKey only, so it is registered without encrypt / decrypt.
  defineAlgorithm("AES-KW", ["generateKey", "importKey", "wrapKey", "unwrapKey", "get key length"],
    aesFamily("AES-KW", "KW", async (direction, alg, key, data) => {
      const [out] = await call("aes_kw", ascii(direction), materialOf(key), data);
      return out;
    }, ["wrapKey", "unwrapKey"]));

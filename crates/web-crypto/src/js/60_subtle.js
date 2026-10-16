
  // ---- SubtleCrypto methods -------------------------------------------------------------------------
  // Each method is async, so argument errors reject; BufferSource arguments are copied before the first await.
  const FORMATS = ["raw", "pkcs8", "spki", "jwk"];

  function checkFormat(format) {
    if (!FORMATS.includes(format)) {
      throw new TypeError(`The provided value '${format}' is not a valid enum value of type KeyFormat`);
    }
  }

  function brandKey(key) {
    if (!(key instanceof CryptoKey)) throw new TypeError("key is not a CryptoKey");
    keyRecord(key);
  }

  function checkKeyUse(alg, key, usage) {
    if (key.algorithm.name !== alg.name) {
      throw domError("InvalidAccessError", "key.algorithm does not match that of the operation");
    }
    if (!key.usages.includes(usage)) throw domError("InvalidAccessError", `key.usages does not permit ${usage}`);
  }

  subtle.generateKey = async function generateKey(algorithm, extractable, keyUsages) {
    const alg = normalizeAlgorithm(algorithm, "generateKey");
    return IMPL.get(alg.name).generate(alg, Boolean(extractable), toUsages(keyUsages));
  };

  subtle.importKey = async function importKey(format, keyData, algorithm, extractable, keyUsages) {
    format = String(format);
    checkFormat(format);
    const data = format === "jwk" ? jwkObject(keyData) : copyBytes(keyData, "keyData");
    const alg = normalizeAlgorithm(algorithm, "importKey");
    return IMPL.get(alg.name).importKey(format, data, alg, Boolean(extractable), toUsages(keyUsages));
  };

  subtle.exportKey = async function exportKey(format, key) {
    format = String(format);
    checkFormat(format);
    brandKey(key);
    if (!key.extractable) throw domError("InvalidAccessError", "key is not extractable");
    return IMPL.get(key.algorithm.name).exportKey(format, key);
  };

  subtle.sign = async function sign(algorithm, key, data) {
    const bytes = copyBytes(data, "data");
    const alg = normalizeAlgorithm(algorithm, "sign");
    brandKey(key);
    checkKeyUse(alg, key, "sign");
    return IMPL.get(alg.name).sign(alg, key, bytes);
  };

  subtle.verify = async function verify(algorithm, key, signature, data) {
    const sig = copyBytes(signature, "signature");
    const bytes = copyBytes(data, "data");
    const alg = normalizeAlgorithm(algorithm, "verify");
    brandKey(key);
    checkKeyUse(alg, key, "verify");
    return IMPL.get(alg.name).verify(alg, key, sig, bytes);
  };

  for (const op of ["encrypt", "decrypt"]) {
    subtle[op] = {
      async [op](algorithm, key, data) {
        const bytes = copyBytes(data, "data");
        const alg = normalizeAlgorithm(algorithm, op);
        brandKey(key);
        checkKeyUse(alg, key, op);
        return IMPL.get(alg.name)[op](alg, key, bytes);
      },
    }[op];
  }

  // wrapKey / unwrapKey: export (or import) the key, then encrypt (or decrypt) it with the wrapping key.
  subtle.wrapKey = async function wrapKey(format, key, wrappingKey, wrapAlgorithm) {
    format = String(format);
    checkFormat(format);
    const alg = normalizeAlgorithm(wrapAlgorithm, "wrapKey");
    brandKey(key);
    brandKey(wrappingKey);
    checkKeyUse(alg, wrappingKey, "wrapKey");
    const exported = await subtle.exportKey(format, key);
    let bytes = format === "jwk" ? new global.TextEncoder().encode(JSON.stringify(exported)) : new Uint8Array(exported);
    if (alg.name === "AES-KW" && bytes.length % 8 !== 0) {
      // AES-KW needs a multiple of 8 bytes; trailing spaces keep the JSON valid.
      bytes = Uint8Array.from({ length: Math.ceil(bytes.length / 8) * 8 }, (_, i) => bytes[i] ?? 0x20);
    }
    return IMPL.get(alg.name).encrypt(alg, wrappingKey, bytes);
  };

  subtle.unwrapKey = async function unwrapKey(
    format, wrappedKey, unwrappingKey, unwrapAlgorithm, unwrappedKeyAlgorithm, extractable, keyUsages) {
    format = String(format);
    checkFormat(format);
    const wrapped = copyBytes(wrappedKey, "wrappedKey");
    const alg = normalizeAlgorithm(unwrapAlgorithm, "unwrapKey");
    const keyAlg = normalizeAlgorithm(unwrappedKeyAlgorithm, "importKey");
    const usages = toUsages(keyUsages);
    brandKey(unwrappingKey);
    checkKeyUse(alg, unwrappingKey, "unwrapKey");
    const plain = new Uint8Array(await IMPL.get(alg.name).decrypt(alg, unwrappingKey, wrapped));
    let keyData = plain;
    if (format === "jwk") {
      try {
        keyData = jwkObject(JSON.parse(new global.TextDecoder("utf-8", { fatal: true }).decode(plain)));
      } catch (error) {
        throw domError("DataError", "The unwrapped key is not a valid JSON Web Key");
      }
    }
    return IMPL.get(keyAlg.name).importKey(format, keyData, keyAlg, Boolean(extractable), usages);
  };


  // ---- PBKDF2 / HKDF (base keys) and deriveBits / deriveKey ------------------------------------------
  function kdfFamily(name, derive) {
    return {
      async importKey(format, data, alg, extractable, usages) {
        if (format !== "raw") throw unsupportedFormat(format, name);
        checkUsages(usages, ["deriveKey", "deriveBits"]);
        if (extractable) throw domError("SyntaxError", `${name} keys cannot be extractable`);
        requireUsages(usages);
        return createKey({ type: "secret", algorithm: { name }, extractable: false, usages, material: data });
      },

      async deriveBits(alg, key, length) {
        if (length === null || length === undefined || length % 8 !== 0) {
          throw domError("OperationError", `${name} needs a length that is a multiple of 8 bits`);
        }
        const hash = hashName(alg.hash);
        const [out] = await derive(alg, hash, materialOf(key), length / 8);
        return out.buffer;
      },
    };
  }

  const bytesParam = (value, what) => copyBytes(value, what);

  defineAlgorithm("PBKDF2", ["importKey", "deriveBits"], kdfFamily("PBKDF2", (alg, hash, password, size) =>
    call("pbkdf2", ascii(hash), password, bytesParam(alg.salt, "salt"), ascii(String(alg.iterations)), ascii(String(size)))));

  defineAlgorithm("HKDF", ["importKey", "deriveBits"], kdfFamily("HKDF", (alg, hash, ikm, size) =>
    call("hkdf", ascii(hash), ikm, bytesParam(alg.salt, "salt"), bytesParam(alg.info, "info"), ascii(String(size)))));

  subtle.deriveBits = async function deriveBits(algorithm, baseKey, length) {
    const alg = normalizeAlgorithm(algorithm, "deriveBits");
    brandKey(baseKey);
    checkKeyUse(alg, baseKey, "deriveBits");
    return IMPL.get(alg.name).deriveBits(alg, baseKey, length === undefined ? null : length);
  };

  subtle.deriveKey = async function deriveKey(algorithm, baseKey, derivedKeyType, extractable, keyUsages) {
    const alg = normalizeAlgorithm(algorithm, "deriveBits");
    const importAlg = normalizeAlgorithm(derivedKeyType, "importKey");
    const lengthAlg = normalizeAlgorithm(derivedKeyType, "get key length");
    const usages = toUsages(keyUsages);
    brandKey(baseKey);
    checkKeyUse(alg, baseKey, "deriveKey");
    const bits = IMPL.get(lengthAlg.name).getKeyLength(lengthAlg);
    if (bits % 8 !== 0) throw domError("NotSupportedError", "Derived key lengths that are not a multiple of 8 bits are not supported");
    const secret = await IMPL.get(alg.name).deriveBits(alg, baseKey, bits);
    return IMPL.get(importAlg.name).importKey("raw", new Uint8Array(secret), importAlg, Boolean(extractable), usages);
  };


  // ---- RSA: RSA-PSS, RSASSA-PKCS1-v1_5 and RSA-OAEP ---------------------------------------------------
  // Material is `{ privateKey: PKCS#1 RSAPrivateKey, publicKey: PKCS#1 RSAPublicKey }` (DER bytes).
  const RSA_JWK_ALGS = {
    pss: { "SHA-1": "PS1", "SHA-256": "PS256", "SHA-384": "PS384", "SHA-512": "PS512" },
    pkcs1: { "SHA-1": "RS1", "SHA-256": "RS256", "SHA-384": "RS384", "SHA-512": "RS512" },
    oaep: { "SHA-1": "RSA-OAEP", "SHA-256": "RSA-OAEP-256", "SHA-384": "RSA-OAEP-384", "SHA-512": "RSA-OAEP-512" },
  };
  const RSA_PRIVATE_MEMBERS = ["d", "p", "q", "dp", "dq", "qi"];

  function rsaAlgorithm(name, hash, [n, e]) {
    return { name, modulusLength: (n.length - 1) * 8 + (32 - Math.clz32(n[0])), publicExponent: e, hash: { name: hash } };
  }

  function rsaFamily(name, kind, publicOps, privateOps, use, operations) {
    const allowed = [...publicOps, ...privateOps];
    const jwkAlgs = RSA_JWK_ALGS[kind];
    const build = async (type, hash, extractable, usages, material) => {
      const components = await call("rsa_components", material.publicKey);
      return createKey({ type, algorithm: rsaAlgorithm(name, hash, components), extractable, usages, material });
    };
    const family = {
      async generate(alg, extractable, usages) {
        const hash = hashName(alg.hash);
        if (!Number.isInteger(alg.modulusLength)) throw new TypeError("algorithm.modulusLength is required");
        const exponent = copyBytes(alg.publicExponent, "algorithm.publicExponent");
        checkUsages(usages, allowed);
        const privateUsages = usages.filter((usage) => privateOps.includes(usage));
        requireUsages(privateUsages);
        const [privateKey, publicKey] = await call("rsa_generate", ascii(String(alg.modulusLength)), exponent);
        const material = { privateKey, publicKey };
        return {
          publicKey: await build("public", hash, true, usages.filter((usage) => publicOps.includes(usage)), { publicKey }),
          privateKey: await build("private", hash, extractable, privateUsages, material),
        };
      },

      async importKey(format, data, alg, extractable, usages) {
        const hash = hashName(alg.hash);
        let type = "public";
        let material;
        if (format === "spki") {
          checkUsages(usages, publicOps);
          material = { publicKey: (await call("rsa_decode", ascii("spki"), data))[0] };
        } else if (format === "pkcs8") {
          checkUsages(usages, privateOps);
          requireUsages(usages);
          const [privateKey, publicKey] = await call("rsa_decode", ascii("pkcs8"), data);
          type = "private";
          material = { privateKey, publicKey };
        } else if (format === "jwk") {
          const isPrivate = data.d !== undefined;
          checkUsages(usages, isPrivate ? privateOps : publicOps);
          if (isPrivate) requireUsages(usages);
          checkJwk(data, { kty: "RSA", use, usages, extractable });
          if (data.oth !== undefined) throw domError("NotSupportedError", "RSA keys with more than two primes are not supported");
          if (data.alg !== undefined && data.alg !== jwkAlgs[hash]) {
            throw domError("DataError", "JWK alg does not match the algorithm");
          }
          const members = ["n", "e", ...(isPrivate ? RSA_PRIVATE_MEMBERS : [])];
          const parts = members.map((member) => base64urlDecode(data[member], `JWK ${member}`));
          const [first, second] = await call("rsa_assemble", ...parts);
          type = isPrivate ? "private" : "public";
          material = isPrivate ? { privateKey: first, publicKey: second } : { publicKey: first };
        } else {
          throw unsupportedFormat(format, name);
        }
        return build(type, hash, extractable, usages, material);
      },

      async exportKey(format, key) {
        const { privateKey, publicKey } = materialOf(key);
        if (format === "spki" && key.type !== "public") throw domError("InvalidAccessError", "spki export needs a public key");
        if (format === "pkcs8" && key.type !== "private") throw domError("InvalidAccessError", "pkcs8 export needs a private key");
        if (format === "spki" || format === "pkcs8") {
          const [der] = await call("rsa_encode", ascii(format), privateKey ?? new Uint8Array(0), publicKey);
          return der.buffer;
        }
        if (format !== "jwk") throw unsupportedFormat(format, name);
        const names = key.type === "private" ? ["n", "e", ...RSA_PRIVATE_MEMBERS] : ["n", "e"];
        const values = await call("rsa_components", privateKey ?? publicKey);
        const jwk = { kty: "RSA" };
        names.forEach((member, i) => { jwk[member] = base64urlEncode(values[i]); });
        return { ...jwk, alg: jwkAlgs[key.algorithm.hash.name], ...jwkBase(key) };
      },
    };
    return Object.assign(family, operations);
  }

  // PSS only supports a salt as long as the digest, which Rust enforces; here the parameter is only parsed.
  function saltOf(alg) {
    if (!Number.isInteger(alg.saltLength) || alg.saltLength < 0) throw new TypeError("algorithm.saltLength is required");
    return ascii(String(alg.saltLength));
  }

  const rsaSigning = (kind) => ({
    async sign(alg, key, data) {
      const salt = kind === "pss" ? saltOf(alg) : new Uint8Array(0);
      const [sig] = await call("rsa_sign", ascii(kind), ascii(key.algorithm.hash.name), materialOf(key).privateKey, data, salt);
      return sig.buffer;
    },
    async verify(alg, key, signature, data) {
      const salt = kind === "pss" ? saltOf(alg) : new Uint8Array(0);
      const [ok] = await call("rsa_verify", ascii(kind), ascii(key.algorithm.hash.name), materialOf(key).publicKey, signature, data, salt);
      return ok[0] === 1;
    },
  });

  const rsaOaep = {
    async encrypt(alg, key, data) {
      const label = alg.label === undefined ? new Uint8Array(0) : copyBytes(alg.label, "algorithm.label");
      const [out] = await call("rsa_oaep", ascii("encrypt"), ascii(key.algorithm.hash.name), materialOf(key).publicKey, data, label);
      return out.buffer;
    },
    async decrypt(alg, key, data) {
      const label = alg.label === undefined ? new Uint8Array(0) : copyBytes(alg.label, "algorithm.label");
      const [out] = await call("rsa_oaep", ascii("decrypt"), ascii(key.algorithm.hash.name), materialOf(key).privateKey, data, label);
      return out.buffer;
    },
  };

  const RSA_SIGN_OPS = [...KEY_OPS, "sign", "verify"];
  defineAlgorithm("RSA-PSS", RSA_SIGN_OPS, rsaFamily("RSA-PSS", "pss", ["verify"], ["sign"], "sig", rsaSigning("pss")));
  defineAlgorithm("RSASSA-PKCS1-v1_5", RSA_SIGN_OPS,
    rsaFamily("RSASSA-PKCS1-v1_5", "pkcs1", ["verify"], ["sign"], "sig", rsaSigning("pkcs1")));
  defineAlgorithm("RSA-OAEP", [...KEY_OPS, "encrypt", "decrypt", "wrapKey", "unwrapKey"],
    rsaFamily("RSA-OAEP", "oaep", ["encrypt", "wrapKey"], ["decrypt", "unwrapKey"], "enc", rsaOaep));

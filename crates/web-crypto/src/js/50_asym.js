
  // ---- Asymmetric keys: ECDSA / ECDH (P-256, P-384) and Ed25519 / X25519 ------------------------------
  // Material is raw: `privateKey` is the EC scalar or the curve25519 seed, `publicKey` the uncompressed point
  // or the 32-byte key. `shape` holds what differs between the EC and OKP JWK/algorithm layouts.
  const CURVE_SIZE = { "P-256": 32, "P-384": 48, "P-521": 66 };

  const ecShape = {
    kty: "EC",
    curveOf(alg) {
      if (alg.namedCurve === undefined) throw new TypeError("algorithm.namedCurve is required");
      if (!Object.hasOwn(CURVE_SIZE, alg.namedCurve)) {
        throw domError("NotSupportedError", `Unsupported named curve ${alg.namedCurve}`);
      }
      return alg.namedCurve;
    },
    algorithm: (name, curve) => ({ name, namedCurve: curve }),
    curveOfKey: (key) => key.algorithm.namedCurve,
    privateSize: (curve) => CURVE_SIZE[curve],
    publicToJwk(curve, point) {
      const size = CURVE_SIZE[curve];
      return { crv: curve, x: base64urlEncode(point.subarray(1, 1 + size)), y: base64urlEncode(point.subarray(1 + size)) };
    },
    publicFromJwk(jwk, curve) {
      const size = CURVE_SIZE[curve];
      const x = base64urlDecode(jwk.x, "JWK x");
      const y = base64urlDecode(jwk.y, "JWK y");
      if (x.length !== size || y.length !== size) throw domError("DataError", "Invalid JWK coordinate length");
      return concatBytes(new Uint8Array([4]), x, y);
    },
  };

  const okpShape = (name) => ({
    kty: "OKP",
    curveOf: () => name,
    algorithm: () => ({ name }),
    curveOfKey: () => name,
    privateSize: () => 32,
    publicToJwk: (curve, key) => ({ crv: curve, x: base64urlEncode(key) }),
    publicFromJwk(jwk) {
      const key = base64urlDecode(jwk.x, "JWK x");
      if (key.length !== 32) throw domError("DataError", "Invalid JWK x length");
      return key;
    },
  });

  // `jwkAlgs(curve)` lists the `alg` values a JWK may carry for this algorithm (null: not validated).
  function asymFamily(name, shape, publicOps, privateOps, use, signing, jwkAlgs = null) {
    const allowed = [...publicOps, ...privateOps];
    const empty = new Uint8Array(0);
    const family = {
      async generate(alg, extractable, usages) {
        const curve = shape.curveOf(alg);
        checkUsages(usages, allowed);
        const privateUsages = usages.filter((usage) => privateOps.includes(usage));
        requireUsages(privateUsages);
        const [privateKey, publicKey] = await call("key_generate", ascii(curve));
        const algorithm = shape.algorithm(name, curve);
        return {
          publicKey: createKey({
            type: "public", algorithm: { ...algorithm }, extractable: true,
            usages: usages.filter((usage) => publicOps.includes(usage)), material: { publicKey },
          }),
          privateKey: createKey({
            type: "private", algorithm, extractable, usages: privateUsages, material: { privateKey, publicKey },
          }),
        };
      },

      async importKey(format, data, alg, extractable, usages) {
        const curve = shape.curveOf(alg);
        const kind = ascii(curve);
        let type = "public";
        let material;
        if (format === "spki") {
          checkUsages(usages, publicOps);
          material = { publicKey: (await call("key_decode", kind, ascii("spki"), data))[0] };
        } else if (format === "pkcs8") {
          checkUsages(usages, privateOps);
          requireUsages(usages);
          const [privateKey, publicKey] = await call("key_decode", kind, ascii("pkcs8"), data);
          type = "private";
          material = { privateKey, publicKey };
        } else if (format === "raw") {
          checkUsages(usages, publicOps);
          await call("key_check_public", kind, data);
          material = { publicKey: data };
        } else if (format === "jwk") {
          const isPrivate = data.d !== undefined;
          checkUsages(usages, isPrivate ? privateOps : publicOps);
          if (isPrivate) requireUsages(usages);
          checkJwk(data, { kty: shape.kty, use, usages, extractable });
          if (data.crv !== curve) throw domError("DataError", "JWK crv does not match the algorithm");
          if (data.alg !== undefined && jwkAlgs !== null && !jwkAlgs(curve).includes(data.alg)) {
            throw domError("DataError", "JWK alg does not match the algorithm");
          }
          const publicKey = shape.publicFromJwk(data, curve);
          await call("key_check_public", kind, publicKey);
          material = { publicKey };
          if (isPrivate) {
            const privateKey = base64urlDecode(data.d, "JWK d");
            if (privateKey.length !== shape.privateSize(curve)) throw domError("DataError", "Invalid JWK d length");
            const [derived] = await call("key_public", kind, privateKey);
            if (!sameBytes(derived, publicKey)) throw domError("DataError", "JWK d does not match the public key");
            type = "private";
            material = { privateKey, publicKey };
          }
        } else {
          throw unsupportedFormat(format, name);
        }
        const algorithm = shape.algorithm(name, curve);
        return createKey({ type, algorithm, extractable, usages, material });
      },

      async exportKey(format, key) {
        const { privateKey, publicKey } = materialOf(key);
        const curve = shape.curveOfKey(key);
        const wantPublic = format === "spki" || format === "raw";
        if (wantPublic && key.type !== "public") throw domError("InvalidAccessError", `${format} export needs a public key`);
        if (format === "pkcs8" && key.type !== "private") throw domError("InvalidAccessError", "pkcs8 export needs a private key");
        if (format === "raw") return publicKey.slice().buffer;
        if (format === "spki" || format === "pkcs8") {
          const [der] = await call("key_encode", ascii(curve), ascii(format), privateKey ?? empty, publicKey);
          return der.buffer;
        }
        if (format !== "jwk") throw unsupportedFormat(format, name);
        const jwk = { kty: shape.kty, ...shape.publicToJwk(curve, publicKey), ...jwkBase(key) };
        if (privateKey !== undefined) jwk.d = base64urlEncode(privateKey);
        return jwk;
      },
    };
    return Object.assign(family, signing);
  }

  const ecdsaSigning = {
    async sign(alg, key, data) {
      const { privateKey, publicKey } = materialOf(key);
      const [sig] = await call("ecdsa_sign", ascii(key.algorithm.namedCurve), ascii(hashName(alg.hash)), privateKey, publicKey, data);
      return sig.buffer;
    },
    async verify(alg, key, signature, data) {
      const [ok] = await call("ecdsa_verify", ascii(key.algorithm.namedCurve), ascii(hashName(alg.hash)), materialOf(key).publicKey, signature, data);
      return ok[0] === 1;
    },
  };

  const ed25519Signing = {
    async sign(alg, key, data) {
      const [sig] = await call("ed25519_sign", materialOf(key).privateKey, data);
      return sig.buffer;
    },
    async verify(alg, key, signature, data) {
      const [ok] = await call("ed25519_verify", materialOf(key).publicKey, signature, data);
      return ok[0] === 1;
    },
  };

  // ECDH and X25519: the peer's public key travels in `alg.public`; the shared secret is truncated to `length` bits.
  const agreementDeriving = {
    async deriveBits(alg, key, length) {
      const peer = alg.public;
      if (!(peer instanceof CryptoKey)) throw new TypeError("algorithm.public must be a CryptoKey");
      const mine = key.algorithm;
      if (peer.type !== "public") throw domError("InvalidAccessError", "algorithm.public must be a public key");
      if (peer.algorithm.name !== mine.name || peer.algorithm.namedCurve !== mine.namedCurve) {
        throw domError("InvalidAccessError", "algorithm.public does not match the private key's algorithm");
      }
      const curve = mine.namedCurve ?? mine.name;
      const [secret] = await call("ecdh_derive", ascii(curve), materialOf(key).privateKey, materialOf(peer).publicKey);
      if (length === null || length === undefined) return secret.buffer;
      if (length > secret.length * 8) throw domError("OperationError", "The length is greater than the shared secret");
      const out = secret.slice(0, Math.ceil(length / 8));
      if (length % 8 !== 0) out[out.length - 1] &= 0xff << (8 - (length % 8));
      return out.buffer;
    },
  };

  const KEY_OPS = ["generateKey", "importKey"];
  const ECDSA_JWK_ALG = { "P-256": "ES256", "P-384": "ES384", "P-521": "ES512" };
  defineAlgorithm("ECDSA", [...KEY_OPS, "sign", "verify"],
    asymFamily("ECDSA", ecShape, ["verify"], ["sign"], "sig", ecdsaSigning, (curve) => [ECDSA_JWK_ALG[curve]]));
  defineAlgorithm("Ed25519", [...KEY_OPS, "sign", "verify"],
    asymFamily("Ed25519", okpShape("Ed25519"), ["verify"], ["sign"], "sig", ed25519Signing, () => ["Ed25519", "EdDSA"]));
  defineAlgorithm("ECDH", [...KEY_OPS, "deriveBits"],
    asymFamily("ECDH", ecShape, [], ["deriveKey", "deriveBits"], "enc", agreementDeriving));
  defineAlgorithm("X25519", [...KEY_OPS, "deriveBits"],
    asymFamily("X25519", okpShape("X25519"), [], ["deriveKey", "deriveBits"], "enc", agreementDeriving));

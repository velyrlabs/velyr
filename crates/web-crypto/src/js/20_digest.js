
  // ---- digest -------------------------------------------------------------------------------------
  for (const hash of ["SHA-1", "SHA-256", "SHA-384", "SHA-512"]) register(hash, ["digest"]);

  subtle.digest = async function digest(algorithm, data) {
    const bytes = copyBytes(data, "data");
    const { name } = normalizeAlgorithm(algorithm, "digest");
    const [out] = await call("digest", ascii(name), bytes);
    return out.buffer;
  };

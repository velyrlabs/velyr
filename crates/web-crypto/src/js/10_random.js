
  // ---- getRandomValues / randomUUID ---------------------------------------------------------------
  const MAX_RANDOM_BYTES = 65536;
  const typedArrayTag = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag).get;
  const INTEGER_ARRAYS = new Set([
    "Int8Array", "Uint8Array", "Uint8ClampedArray", "Int16Array", "Uint16Array",
    "Int32Array", "Uint32Array", "BigInt64Array", "BigUint64Array",
  ]);

  function randomBytes(length) {
    const text = randomOp(length);
    const bytes = new Uint8Array(length);
    for (let i = 0; i < length; i++) bytes[i] = text.charCodeAt(i);
    return bytes;
  }

  function getRandomValues(array) {
    if (!INTEGER_ARRAYS.has(typedArrayTag.call(array))) {
      throw domError("TypeMismatchError", "The data argument must be an integer-type TypedArray");
    }
    if (array.byteLength > MAX_RANDOM_BYTES) {
      throw domError("QuotaExceededError",
        `The ArrayBufferView's byte length (${array.byteLength}) exceeds the number of bytes of entropy available via this API (${MAX_RANDOM_BYTES})`);
    }
    new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(randomBytes(array.byteLength));
    return array;
  }

  function randomUUID() {
    const b = randomBytes(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    const hex = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  }

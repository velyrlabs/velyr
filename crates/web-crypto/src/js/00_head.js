// Bootstrap: WebCrypto (crypto.getRandomValues, randomUUID, crypto.subtle) over the private op_crypto_* bindings.
// lib.rs concatenates 00..90 into one script: this file opens the private scope and 90_exports.js closes it.
// Keys are plain byte strings held in a private record per CryptoKey; Rust only runs stateless primitives.
(function (global) {
  "use strict";

  // Capture the raw ops, then remove them so user code never sees them (ADR-010).
  const randomOp = global.op_crypto_random;
  const runOp = global.op_crypto_run;
  delete global.op_crypto_random;
  delete global.op_crypto_run;

  const CHUNK = 0x2000;
  const subtle = {}; // algorithm-independent method table, filled by the fragments, wrapped in 90_exports.js

  function domError(name, message) {
    return new global.DOMException(message, name);
  }

  // Rust reports failures as "<DOMException name>: <message>".
  function translate(error) {
    const match = /^(\w+Error): ([\s\S]*)$/.exec(error && error.message);
    return match ? domError(match[1], match[2]) : error;
  }

  function toBinary(bytes) {
    let out = "";
    for (let i = 0; i < bytes.length; i += CHUNK) {
      out += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
    }
    return out;
  }

  function ascii(text) {
    const bytes = new Uint8Array(text.length);
    for (let i = 0; i < text.length; i++) bytes[i] = text.charCodeAt(i);
    return bytes;
  }

  function frame(parts) {
    let out = "";
    for (const part of parts) {
      const n = part.length;
      out += String.fromCharCode((n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255) + toBinary(part);
    }
    return out;
  }

  function unframe(buffer) {
    const bytes = new Uint8Array(buffer);
    const parts = [];
    for (let at = 0; at < bytes.length;) {
      const n = ((bytes[at] << 24) | (bytes[at + 1] << 16) | (bytes[at + 2] << 8) | bytes[at + 3]) >>> 0;
      parts.push(bytes.slice(at + 4, at + 4 + n));
      at += 4 + n;
    }
    return parts;
  }

  // Runs a Rust primitive; `parts` are Uint8Arrays, the result is the list of Uint8Array outputs.
  async function call(name, ...parts) {
    try {
      return unframe(await runOp(name, frame(parts)));
    } catch (error) {
      throw translate(error);
    }
  }

  // Copies a BufferSource so later mutation by the caller cannot change an in-flight operation.
  function copyBytes(data, what) {
    if (ArrayBuffer.isView(data)) {
      return new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
    }
    if (data instanceof ArrayBuffer) return new Uint8Array(data.slice(0));
    throw new TypeError(`${what} must be an ArrayBuffer or ArrayBufferView`);
  }

  const ALGORITHMS = new Map(); // upper-case name -> { name, ops }
  function register(name, ops) {
    ALGORITHMS.set(name.toUpperCase(), { name, ops: new Set(ops) });
  }

  // Resolves an AlgorithmIdentifier for `op`: unknown names and unsupported operations are NotSupportedError.
  function normalizeAlgorithm(algorithm, op) {
    const init = typeof algorithm === "string" ? { name: algorithm } : algorithm;
    if (init === null || typeof init !== "object") {
      throw new TypeError("algorithm must be a string or an object");
    }
    if (init.name === undefined) throw new TypeError("algorithm.name is required");
    const name = String(init.name);
    const entry = ALGORITHMS.get(name.toUpperCase());
    if (entry === undefined || !entry.ops.has(op)) {
      throw domError("NotSupportedError", `Unrecognized algorithm name ${name} for ${op}`);
    }
    return { ...init, name: entry.name };
  }

// Bootstrap: TextEncoder, TextDecoder, atob and btoa on top of the private op_encoding_* binding.
// Evaluated once per isolate. Transcoding lives in Rust (encoding_rs); bytes cross the boundary as
// binary strings (one byte per UTF-16 unit) in, and as ArrayBuffers out.
(function (global) {
  "use strict";

  // Capture the raw ops, then remove them so user code never sees them (ADR-010).
  const labelOp = global.op_encoding_label;
  const decodeOp = global.op_encoding_decode;
  const encodeOp = global.op_encoding_encode;
  const encodeIntoOp = global.op_encoding_encode_into;
  delete global.op_encoding_label;
  delete global.op_encoding_decode;
  delete global.op_encoding_encode;
  delete global.op_encoding_encode_into;

  function define(name, value) {
    Object.defineProperty(global, name, {
      value,
      writable: true,
      configurable: true,
      enumerable: false,
    });
  }

  const CHUNK = 0x2000;

  function toBinaryString(bytes) {
    let out = "";
    for (let i = 0; i < bytes.length; i += CHUNK) {
      out += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
    }
    return out;
  }

  function toBytes(input) {
    if (input === undefined) return new Uint8Array(0);
    if (ArrayBuffer.isView(input)) {
      return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
    }
    if (input instanceof ArrayBuffer || input instanceof SharedArrayBuffer) {
      return new Uint8Array(input);
    }
    throw new TypeError("The provided value is not an ArrayBuffer or ArrayBufferView");
  }

  // ---- TextEncoder ----

  class TextEncoder {
    get encoding() { return "utf-8"; }

    encode(input = "") {
      return new Uint8Array(encodeOp(String(input)));
    }

    encodeInto(source, destination) {
      if (!(destination instanceof Uint8Array)) {
        throw new TypeError("encodeInto destination must be a Uint8Array");
      }
      const [read, buffer] = encodeIntoOp(String(source), destination.length);
      const bytes = new Uint8Array(buffer);
      destination.set(bytes);
      return { read, written: bytes.length };
    }

    get [Symbol.toStringTag]() { return "TextEncoder"; }
  }

  // ---- TextDecoder ----

  let nextStreamId = 1;

  class TextDecoder {
    #name;
    #fatal;
    #ignoreBOM;
    #streamId = 0;

    constructor(label = "utf-8", options = {}) {
      const name = labelOp(String(label));
      if (name === null) throw new RangeError(`The encoding label provided ('${label}') is invalid.`);
      this.#name = name;
      this.#fatal = Boolean(options?.fatal);
      this.#ignoreBOM = Boolean(options?.ignoreBOM);
    }

    get encoding() { return this.#name; }
    get fatal() { return this.#fatal; }
    get ignoreBOM() { return this.#ignoreBOM; }

    decode(input = undefined, options = {}) {
      const bytes = toBytes(input);
      const stream = Boolean(options?.stream);
      if (stream && this.#streamId === 0) this.#streamId = nextStreamId++;
      const id = this.#streamId;
      // A final or failed call drops the Rust-side decoder, so the next call starts fresh.
      if (!stream) this.#streamId = 0;
      try {
        return decodeOp(this.#name, toBinaryString(bytes), this.#fatal, this.#ignoreBOM, stream, id);
      } catch (error) {
        this.#streamId = 0;
        throw error;
      }
    }

    get [Symbol.toStringTag]() { return "TextDecoder"; }
  }

  // ---- atob / btoa (WHATWG forgiving-base64) ----

  const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

  // DOMException is looked up at throw time so load order does not matter; without the events
  // bootstrap (shortcut) it falls back to a plain Error with the same name and code.
  function invalidCharacter(message) {
    if (typeof globalThis.DOMException === "function") {
      return new globalThis.DOMException(message, "InvalidCharacterError");
    }
    const error = new Error(message);
    error.name = "InvalidCharacterError";
    error.code = 5;
    return error;
  }

  function btoa(data) {
    if (arguments.length === 0) throw new TypeError("btoa requires 1 argument");
    const input = String(data);
    let out = "";
    for (let i = 0; i < input.length; i += 3) {
      const units = [input.charCodeAt(i), input.charCodeAt(i + 1), input.charCodeAt(i + 2)];
      if (units.some((unit) => unit > 0xff)) {
        throw invalidCharacter("The string to be encoded contains characters outside of the Latin1 range.");
      }
      const present = Math.min(3, input.length - i);
      const triple = (units[0] << 16) | ((units[1] || 0) << 8) | (units[2] || 0);
      out += ALPHABET[triple >> 18] + ALPHABET[(triple >> 12) & 63];
      out += present > 1 ? ALPHABET[(triple >> 6) & 63] : "=";
      out += present > 2 ? ALPHABET[triple & 63] : "=";
    }
    return out;
  }

  function atob(data) {
    if (arguments.length === 0) throw new TypeError("atob requires 1 argument");
    let input = String(data).replace(/[\t\n\f\r ]/g, "");
    if (input.length % 4 === 0) input = input.replace(/==?$/, "");
    if (input.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(input)) {
      throw invalidCharacter("The string to be decoded is not correctly encoded.");
    }
    let out = "";
    let buffer = 0, bits = 0;
    for (const ch of input) {
      buffer = (buffer << 6) | ALPHABET.indexOf(ch);
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out += String.fromCharCode((buffer >> bits) & 0xff);
      }
    }
    return out;
  }

  define("TextEncoder", TextEncoder);
  define("TextDecoder", TextDecoder);
  define("atob", atob);
  define("btoa", btoa);
})(globalThis);

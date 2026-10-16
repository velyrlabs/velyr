// Blob, File and blob: object URLs on top of the private op_blob_* binding. Evaluated once per
// isolate, after url.js. The bytes live in Rust; a Blob holds an id, a size and a type, and
// slice() makes a new id over the same bytes.
(function (global) {
  "use strict";

  const createOp = global.op_blob_create;
  const sliceOp = global.op_blob_slice;
  const readOp = global.op_blob_read;
  const freeOp = global.op_blob_free;
  for (const name of ["create", "slice", "read", "free"]) delete global[`op_blob_${name}`];

  const STREAM_CHUNK = 64 * 1024;
  const INTERNAL = Symbol("velyr.blob.internal");
  const blobs = new WeakMap();
  const owners = new FinalizationRegistry((id) => freeOp(id));
  const objectUrls = new Map();

  function define(name, value) {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  }

  function normalizeType(value) {
    const type = String(value ?? "");
    return /^[\x20-\x7e]*$/.test(type) ? type.toLowerCase() : "";
  }

  function register(blob, id, size, type) {
    blobs.set(blob, { id, size, type });
    owners.register(blob, id);
  }

  function stateOf(blob) {
    const state = blobs.get(blob);
    if (state === undefined) throw new TypeError("Illegal invocation");
    return state;
  }

  const isBlob = (value) => value !== null && typeof value === "object" && blobs.has(value);

  function readBytes(blob) {
    const { id, size } = stateOf(blob);
    return new Uint8Array(readOp(id, 0, size));
  }

  function partBytes(part) {
    if (isBlob(part)) return readBytes(part);
    if (part instanceof ArrayBuffer) return new Uint8Array(part);
    if (ArrayBuffer.isView(part)) return new Uint8Array(part.buffer, part.byteOffset, part.byteLength);
    return new TextEncoder().encode(String(part).toWellFormed());
  }

  function joinParts(parts) {
    const chunks = Array.from(parts, partBytes);
    if (chunks.length === 1) return chunks[0];
    const all = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.byteLength, 0));
    let offset = 0;
    for (const chunk of chunks) {
      all.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return all;
  }

  // Clamps a slice() bound the way the File API does: negative counts from the end.
  function bound(value, size, fallback) {
    if (value === undefined) return fallback;
    const number = Math.trunc(Number(value)) || 0;
    return number < 0 ? Math.max(size + number, 0) : Math.min(number, size);
  }

  class Blob {
    constructor(parts = undefined, options = undefined) {
      if (parts === INTERNAL) {
        register(this, options.id, options.size, options.type);
        return;
      }
      if (parts !== undefined) {
        if (parts === null || typeof parts !== "object" || typeof parts[Symbol.iterator] !== "function") {
          throw new TypeError("Blob parts must be an iterable of strings, buffers or Blobs");
        }
      }
      if (options !== undefined && options !== null && typeof options !== "object" && typeof options !== "function") {
        throw new TypeError("Blob options must be an object");
      }
      const bytes = parts === undefined ? new Uint8Array(0) : joinParts(parts);
      register(this, createOp(bytes), bytes.byteLength, normalizeType(options?.type));
    }

    get size() { return stateOf(this).size; }
    get type() { return stateOf(this).type; }

    slice(start = undefined, end = undefined, contentType = undefined) {
      const { id, size } = stateOf(this);
      const from = bound(start, size, 0);
      const to = Math.max(bound(end, size, size), from);
      return new Blob(INTERNAL, { id: sliceOp(id, from, to), size: to - from, type: normalizeType(contentType) });
    }

    async arrayBuffer() {
      const { id, size } = stateOf(this);
      return readOp(id, 0, size);
    }

    async bytes() {
      return new Uint8Array(await this.arrayBuffer());
    }

    async text() {
      return new TextDecoder().decode(await this.arrayBuffer());
    }

    stream() {
      const blob = this; // referenced in pull(), so the bytes stay alive while the stream is read
      let offset = 0;
      return new ReadableStream({
        type: "bytes",
        pull(controller) {
          const { id, size } = stateOf(blob);
          if (offset >= size) {
            controller.close();
            controller.byobRequest?.respond(0);
            return;
          }
          const length = Math.min(STREAM_CHUNK, size - offset);
          controller.enqueue(new Uint8Array(readOp(id, offset, length)));
          offset += length;
        },
      });
    }
  }
  Object.defineProperty(Blob.prototype, Symbol.toStringTag, { value: "Blob", configurable: true });

  class File extends Blob {
    #name;
    #lastModified;

    constructor(fileBits, fileName, options = undefined) {
      if (arguments.length < 2) throw new TypeError("File requires 2 arguments");
      super(fileBits, options);
      this.#name = String(fileName).toWellFormed();
      const modified = options?.lastModified;
      this.#lastModified = modified === undefined ? Date.now() : Math.trunc(Number(modified)) || 0;
    }

    get name() { return this.#name; }
    get lastModified() { return this.#lastModified; }
  }
  Object.defineProperty(File.prototype, Symbol.toStringTag, { value: "File", configurable: true });

  for (const Class of [Blob, File]) {
    for (const [name, descriptor] of Object.entries(Object.getOwnPropertyDescriptors(Class.prototype))) {
      if (name !== "constructor") {
        Object.defineProperty(Class.prototype, name, { ...descriptor, enumerable: true });
      }
    }
  }

  function randomId() {
    const hex = () => Math.floor(Math.random() * 0x100000000).toString(16).padStart(8, "0");
    return `${hex()}-${hex().slice(0, 4)}-${hex().slice(0, 4)}-${hex().slice(0, 4)}-${hex()}${hex().slice(0, 4)}`;
  }

  // shortcut: the registry holds the Blob until revokeObjectURL, as the standard says; there is
  // no per-origin partitioning, so every URL is `blob:null/<id>`.
  Object.defineProperties(global.URL, {
    createObjectURL: {
      value: function createObjectURL(object) {
        if (!isBlob(object)) throw new TypeError("URL.createObjectURL expects a Blob");
        const url = `blob:null/${randomId()}`;
        objectUrls.set(url, object);
        return url;
      },
      writable: true,
      configurable: true,
    },
    revokeObjectURL: {
      value: function revokeObjectURL(url) {
        objectUrls.delete(String(url).replace(/#.*$/s, ""));
      },
      writable: true,
      configurable: true,
    },
  });

  // Reachable only as globalThis[Symbol.for("velyr.internal.blob")] (non-enumerable); fetch and
  // the body mixin use it to read a Blob synchronously and to resolve blob: URLs.
  Object.defineProperty(global, Symbol.for("velyr.internal.blob"), {
    value: Object.freeze({
      isBlob,
      readBytes,
      typeOf: (blob) => stateOf(blob).type,
      objectURL: (url) => objectUrls.get(String(url).replace(/#.*$/s, "")),
    }),
    enumerable: false,
    configurable: false,
    writable: false,
  });

  define("Blob", Blob);
  define("File", File);
})(globalThis);

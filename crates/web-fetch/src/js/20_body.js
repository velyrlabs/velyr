
  // ---- Body ------------------------------------------------------------------------------

  // What backs a Request or Response body: bytes already in memory, or a ReadableStream.
  class BodyState {
    constructor(stream, bytes) {
      this.stream = stream;
      this.bytes = bytes;
      this.used = false;
    }

    // "Disturbed": consumed through this API, locked, or read from or cancelled directly.
    get unusable() {
      return this.used || this.stream?.locked === true || (this.stream !== null && isDisturbed(this.stream));
    }

    get length() {
      return this.bytes === null ? null : this.bytes.byteLength;
    }
  }

  // A body read through native ops.
  function nativeBody(id, read, close) {
    return new BodyState(createNativeReadable(id, read, close), null);
  }

  const bodies = new WeakMap();
  const blobInternal = global[Symbol.for("velyr.internal.blob")] ?? { isBlob: () => false };
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  function bodyStateOf(owner) {
    if (!bodies.has(owner)) throw new TypeError("Illegal invocation");
    return bodies.get(owner);
  }

  function copyBytes(view) {
    return new Uint8Array(view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength));
  }

  // [BodyState | null, contentType | null] for a BodyInit.
  function extractBody(init) {
    if (init === null || init === undefined) return [null, null];
    if (init instanceof ReadableStream) {
      if (init.locked) throw new TypeError("The body stream is locked");
      return [new BodyState(init, null), null];
    }
    if (blobInternal.isBlob(init)) {
      return [new BodyState(null, blobInternal.readBytes(init)), init.type === "" ? null : init.type];
    }
    if (init instanceof FormData) {
      const { boundary, bytes } = encodeMultipart(init);
      return [new BodyState(null, bytes), `multipart/form-data; boundary=${boundary}`];
    }
    if (init instanceof ArrayBuffer) return [new BodyState(null, new Uint8Array(init.slice(0))), null];
    if (ArrayBuffer.isView(init)) return [new BodyState(null, copyBytes(init)), null];
    if (init instanceof URLSearchParams) {
      return [new BodyState(null, encoder.encode(init.toString())), "application/x-www-form-urlencoded;charset=UTF-8"];
    }
    return [new BodyState(null, encoder.encode(String(init))), "text/plain;charset=UTF-8"];
  }

  async function readAll(stream) {
    const reader = stream.getReader();
    const chunks = [];
    let total = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!(value instanceof Uint8Array)) throw new TypeError("Received a body chunk that is not a Uint8Array");
      chunks.push(value);
      total += value.byteLength;
    }
    const all = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
      all.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return all;
  }

  async function consume(owner) {
    const state = bodyStateOf(owner);
    if (state === null) return new Uint8Array(0);
    if (state.unusable) throw new TypeError("Body is unusable: it was already read");
    state.used = true;
    return state.bytes !== null ? state.bytes : readAll(state.stream);
  }

  function bodyStream(state) {
    if (state.stream === null) {
      const { bytes } = state;
      state.stream = new ReadableStream({
        type: "bytes",
        start(controller) {
          if (bytes.byteLength > 0) controller.enqueue(copyBytes(bytes));
          controller.close();
        },
      });
      state.bytes = null;
    }
    return state.stream;
  }

  // A second, independent state over the same body; the original keeps working too.
  function cloneBody(state) {
    if (state === null) return null;
    if (state.unusable) throw new TypeError("Cannot clone a body that was already read");
    if (state.bytes !== null) return new BodyState(null, state.bytes);
    const [kept, copy] = state.stream.tee();
    state.stream = kept;
    return new BodyState(copy, null);
  }

  const bodyMixin = {
    get body() {
      const state = bodyStateOf(this);
      return state === null ? null : bodyStream(state);
    },
    get bodyUsed() {
      const state = bodyStateOf(this);
      return state !== null && state.unusable;
    },
    async arrayBuffer() {
      const bytes = await consume(this);
      return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
    },
    async bytes() {
      return copyBytes(await consume(this));
    },
    async text() {
      return decoder.decode(await consume(this));
    },
    async json() {
      return JSON.parse(decoder.decode(await consume(this)));
    },
    async formData() {
      return parseFormData(await consume(this), this.headers.get("content-type"));
    },
  };

  // Blob belongs to web-core; blob() appears only where it exists.
  if (typeof global.Blob === "function") {
    bodyMixin.blob = async function blob() {
      const bytes = await consume(this);
      return new global.Blob([bytes], { type: this.headers.get("content-type") ?? "" });
    };
  }

  function mixBody(Class) {
    for (const [name, descriptor] of Object.entries(Object.getOwnPropertyDescriptors(bodyMixin))) {
      Object.defineProperty(Class.prototype, name, { ...descriptor, enumerable: true });
    }
  }

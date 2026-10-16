
  // =====================================================================================
  // TextEncoderStream / TextDecoderStream (on top of the global TextEncoder / TextDecoder)
  // =====================================================================================

  const isHighSurrogate = (unit) => unit >= 0xd800 && unit <= 0xdbff;

  class TextEncoderStream {
    #transform;

    constructor() {
      const encoder = new global.TextEncoder();
      let pendingHighSurrogate = "";
      this.#transform = new TransformStream({
        transform(chunk, controller) {
          let text = pendingHighSurrogate + String(chunk);
          pendingHighSurrogate = "";
          if (text.length > 0 && isHighSurrogate(text.charCodeAt(text.length - 1))) {
            pendingHighSurrogate = text[text.length - 1];
            text = text.slice(0, -1);
          }
          if (text.length > 0) controller.enqueue(encoder.encode(text));
        },
        flush(controller) {
          // A dangling high surrogate encodes as U+FFFD.
          if (pendingHighSurrogate !== "") controller.enqueue(encoder.encode(pendingHighSurrogate));
        },
      });
    }

    get encoding() {
      void this.#transform;
      return "utf-8";
    }

    get readable() {
      return this.#transform.readable;
    }

    get writable() {
      return this.#transform.writable;
    }
  }

  class TextDecoderStream {
    #decoder;
    #transform;

    constructor(label = "utf-8", options = undefined) {
      const decoder = new global.TextDecoder(label, options);
      this.#decoder = decoder;
      this.#transform = new TransformStream({
        transform(chunk, controller) {
          if (!ArrayBuffer.isView(chunk) && !(chunk instanceof ArrayBuffer) && !(chunk instanceof SharedArrayBuffer)) {
            throw new TypeError("The chunk must be an ArrayBuffer or ArrayBufferView");
          }
          const text = decoder.decode(chunk, { stream: true });
          if (text.length > 0) controller.enqueue(text);
        },
        flush(controller) {
          const text = decoder.decode();
          if (text.length > 0) controller.enqueue(text);
        },
      });
    }

    get encoding() {
      return this.#decoder.encoding;
    }

    get fatal() {
      return this.#decoder.fatal;
    }

    get ignoreBOM() {
      return this.#decoder.ignoreBOM;
    }

    get readable() {
      return this.#transform.readable;
    }

    get writable() {
      return this.#transform.writable;
    }
  }

  // =====================================================================================
  // CompressionStream / DecompressionStream (gzip, deflate, deflate-raw via Rust flate2 ops)
  // =====================================================================================

  const FORMATS = ["gzip", "deflate", "deflate-raw"];
  const BINARY_CHUNK = 0x2000;
  let nextFlateId = 1;

  // Bytes cross the Rust boundary as binary strings (one byte per UTF-16 unit).
  function toBinaryString(bytes) {
    let out = "";
    for (let i = 0; i < bytes.length; i += BINARY_CHUNK) {
      out += String.fromCharCode.apply(null, bytes.subarray(i, i + BINARY_CHUNK));
    }
    return out;
  }

  function bufferSourceBytes(chunk) {
    if (ArrayBuffer.isView(chunk)) return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
    if (chunk instanceof ArrayBuffer || chunk instanceof SharedArrayBuffer) return new Uint8Array(chunk);
    throw new TypeError("The chunk must be an ArrayBuffer or ArrayBufferView");
  }

  // The Rust side keeps one codec per id and frees it on the final call, on error and on free().
  function createFlateTransform(compress, format) {
    if (!FORMATS.includes(format)) throw new TypeError(`Unsupported compression format: '${format}'`);
    const id = nextFlateId++;
    const run = (bytes, finish) => {
      const out = new Uint8Array(flateOp(compress, format, id, toBinaryString(bytes), finish));
      return out;
    };
    return new TransformStream({
      transform(chunk, controller) {
        const out = run(bufferSourceBytes(chunk), false);
        if (out.length > 0) controller.enqueue(out);
      },
      flush(controller) {
        const out = run(new Uint8Array(0), true);
        if (out.length > 0) controller.enqueue(out);
      },
      cancel() {
        flateFreeOp(id);
      },
    });
  }

  class CompressionStream {
    #transform;

    constructor(format) {
      this.#transform = createFlateTransform(true, String(format));
    }

    get readable() {
      return this.#transform.readable;
    }

    get writable() {
      return this.#transform.writable;
    }
  }

  class DecompressionStream {
    #transform;

    constructor(format) {
      this.#transform = createFlateTransform(false, String(format));
    }

    get readable() {
      return this.#transform.readable;
    }

    get writable() {
      return this.#transform.writable;
    }
  }

  finishClass(TextEncoderStream, "TextEncoderStream");
  finishClass(TextDecoderStream, "TextDecoderStream");
  finishClass(CompressionStream, "CompressionStream");
  finishClass(DecompressionStream, "DecompressionStream");

  // =====================================================================================
  // Queuing strategies
  // =====================================================================================

  function strategyInit(init) {
    if (Object(init) !== init) throw new TypeError("The strategy init must be an object");
    if (init.highWaterMark === undefined) throw new TypeError("highWaterMark is required");
    return Number(init.highWaterMark);
  }

  const countSize = function size() {
    return 1;
  };
  const byteLengthSize = function size(chunk) {
    return chunk.byteLength;
  };

  class CountQueuingStrategy {
    #highWaterMark;

    constructor(init) {
      this.#highWaterMark = strategyInit(init);
    }

    get highWaterMark() {
      return this.#highWaterMark;
    }

    get size() {
      void this.#highWaterMark;
      return countSize;
    }
  }

  class ByteLengthQueuingStrategy {
    #highWaterMark;

    constructor(init) {
      this.#highWaterMark = strategyInit(init);
    }

    get highWaterMark() {
      return this.#highWaterMark;
    }

    get size() {
      void this.#highWaterMark;
      return byteLengthSize;
    }
  }

  finishClass(CountQueuingStrategy, "CountQueuingStrategy");
  finishClass(ByteLengthQueuingStrategy, "ByteLengthQueuingStrategy");

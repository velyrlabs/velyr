
  // =====================================================================================
  // Public classes: ReadableStream, controllers, readers
  // =====================================================================================

  class ReadableStream {
    constructor(underlyingSource = undefined, strategy = undefined) {
      const stream = newReadableRecord();
      stream.wrapper = this;
      records.set(this, stream);
      const source = toDict(underlyingSource, "underlyingSource");
      const strategyDict = toDict(strategy, "strategy");
      const type = source.type === undefined ? undefined : String(source.type);
      if (type === "bytes") {
        if (strategyDict.size !== undefined) throw new RangeError("The strategy for a byte stream cannot have a size function");
        setUpByteControllerFromSource(stream, source, extractHighWaterMark(strategyDict, 0));
      } else if (type === undefined) {
        const sizeAlgorithm = extractSizeAlgorithm(strategyDict);
        setUpDefaultControllerFromSource(stream, source, extractHighWaterMark(strategyDict, 1), sizeAlgorithm);
      } else {
        throw new TypeError(`Invalid underlyingSource type: ${type}`);
      }
    }

    static from(asyncIterable) {
      return readableStreamFromIterable(asyncIterable);
    }

    get locked() {
      return isLocked(brand(this, "rs", "ReadableStream"));
    }

    cancel(reason = undefined) {
      return guard(() => {
        const stream = brand(this, "rs", "ReadableStream");
        if (isLocked(stream)) return rejected(new TypeError("Cannot cancel a stream that is locked to a reader"));
        return readableStreamCancel(stream, reason);
      });
    }

    getReader(options = undefined) {
      const stream = brand(this, "rs", "ReadableStream");
      const mode = toDict(options, "options").mode;
      if (mode === undefined) return acquireDefaultReader(stream);
      if (String(mode) !== "byob") throw new TypeError("Invalid reader mode");
      return acquireByobReader(stream);
    }

    pipeThrough(transform, options = undefined) {
      const stream = brand(this, "rs", "ReadableStream");
      const pair = toDict(transform, "transform");
      const readable = pair.readable;
      const writable = pair.writable;
      brand(readable, "rs", "ReadableStream");
      const destination = brand(writable, "ws", "WritableStream");
      const pipeOptions = pipeOptionsOf(options);
      if (isLocked(stream)) throw new TypeError("Cannot pipe a locked stream");
      if (destination.writer !== undefined) throw new TypeError("Cannot pipe to a locked stream");
      markHandled(readableStreamPipeTo(stream, destination, pipeOptions));
      return readable;
    }

    pipeTo(destination, options = undefined) {
      return guard(() => {
        const stream = brand(this, "rs", "ReadableStream");
        const target = brand(destination, "ws", "WritableStream");
        const pipeOptions = pipeOptionsOf(options);
        if (isLocked(stream)) throw new TypeError("Cannot pipe a locked stream");
        if (target.writer !== undefined) throw new TypeError("Cannot pipe to a locked stream");
        return readableStreamPipeTo(stream, target, pipeOptions);
      });
    }

    tee() {
      return readableStreamTee(brand(this, "rs", "ReadableStream"));
    }

    values(options = undefined) {
      const stream = brand(this, "rs", "ReadableStream");
      const preventCancel = Boolean(toDict(options, "options").preventCancel);
      return createAsyncIterator(acquireDefaultReader(stream), preventCancel);
    }
  }
  ReadableStream.prototype[Symbol.asyncIterator] = ReadableStream.prototype.values;

  function acquireDefaultReader(stream) {
    const reader = newReader("default-reader");
    const wrapper = construct(ReadableStreamDefaultReader, reader);
    setUpReader(reader, stream);
    return wrapper;
  }

  function acquireByobReader(stream) {
    if (stream.controller.kind !== "brc") throw new TypeError("Only byte streams support BYOB readers");
    const reader = newReader("byob-reader");
    const wrapper = construct(ReadableStreamBYOBReader, reader);
    setUpReader(reader, stream);
    return wrapper;
  }

  class ReadableStreamDefaultController {
    constructor() {
      assertConstructible();
    }

    get desiredSize() {
      return defaultControllerGetDesiredSize(brand(this, "rdc", "ReadableStreamDefaultController"));
    }

    close() {
      const controller = brand(this, "rdc", "ReadableStreamDefaultController");
      if (!defaultControllerCanCloseOrEnqueue(controller)) throw new TypeError("The stream is not in a state that permits close");
      defaultControllerClose(controller);
    }

    enqueue(chunk = undefined) {
      const controller = brand(this, "rdc", "ReadableStreamDefaultController");
      if (!defaultControllerCanCloseOrEnqueue(controller)) throw new TypeError("The stream is not in a state that permits enqueue");
      defaultControllerEnqueue(controller, chunk);
    }

    error(reason = undefined) {
      defaultControllerError(brand(this, "rdc", "ReadableStreamDefaultController"), reason);
    }
  }

  class ReadableStreamDefaultReader {
    constructor(stream) {
      if (allowConstruct) return;
      const reader = newReader("default-reader");
      records.set(this, reader);
      reader.wrapper = this;
      setUpReader(reader, brand(stream, "rs", "ReadableStream"));
    }

    get closed() {
      return guard(() => brand(this, "default-reader", "ReadableStreamDefaultReader").closed.promise);
    }

    cancel(reason = undefined) {
      return guard(() => {
        const reader = brand(this, "default-reader", "ReadableStreamDefaultReader");
        if (reader.stream === undefined) return rejected(new TypeError("Reader was released"));
        return readerCancel(reader, reason);
      });
    }

    read() {
      return guard(() => {
        const reader = brand(this, "default-reader", "ReadableStreamDefaultReader");
        if (reader.stream === undefined) return rejected(new TypeError("Reader was released"));
        const result = defer();
        defaultReaderRead(reader, {
          chunk: (value) => result.resolve({ value, done: false }),
          close: () => result.resolve({ value: undefined, done: true }),
          error: (error) => result.reject(error),
        });
        return result.promise;
      });
    }

    releaseLock() {
      const reader = brand(this, "default-reader", "ReadableStreamDefaultReader");
      if (reader.stream !== undefined) defaultReaderRelease(reader);
    }
  }

  class ReadableStreamBYOBReader {
    constructor(stream) {
      if (allowConstruct) return;
      const target = brand(stream, "rs", "ReadableStream");
      if (target.controller.kind !== "brc") throw new TypeError("Only byte streams support BYOB readers");
      const reader = newReader("byob-reader");
      records.set(this, reader);
      reader.wrapper = this;
      setUpReader(reader, target);
    }

    get closed() {
      return guard(() => brand(this, "byob-reader", "ReadableStreamBYOBReader").closed.promise);
    }

    cancel(reason = undefined) {
      return guard(() => {
        const reader = brand(this, "byob-reader", "ReadableStreamBYOBReader");
        if (reader.stream === undefined) return rejected(new TypeError("Reader was released"));
        return readerCancel(reader, reason);
      });
    }

    read(view, options = undefined) {
      return guard(() => {
        const reader = brand(this, "byob-reader", "ReadableStreamBYOBReader");
        if (!ArrayBuffer.isView(view)) throw new TypeError("view must be an ArrayBufferView");
        if (view.byteLength === 0) throw new TypeError("view must have non-zero byteLength");
        if (view.buffer.byteLength === 0) throw new TypeError("view's buffer must have non-zero byteLength");
        if (isDetached(view.buffer)) throw new TypeError("view's buffer has been detached");
        const min = byobMin(view, toDict(options, "options"));
        if (reader.stream === undefined) throw new TypeError("Reader was released");
        const result = defer();
        byobReaderRead(reader, view, min, {
          chunk: (value) => result.resolve({ value, done: false }),
          close: (value) => result.resolve({ value, done: true }),
          error: (error) => result.reject(error),
        });
        return result.promise;
      });
    }

    releaseLock() {
      const reader = brand(this, "byob-reader", "ReadableStreamBYOBReader");
      if (reader.stream !== undefined) byobReaderRelease(reader);
    }
  }

  function byobMin(view, options) {
    if (options.min === undefined) return 1;
    const min = Number(options.min);
    if (!Number.isInteger(min) || min <= 0) throw new TypeError("options.min must be a positive integer");
    const limit = view instanceof DataView ? view.byteLength : view.length;
    if (min > limit) throw new RangeError("options.min must not exceed the view's length");
    return min;
  }

  finishClass(ReadableStream, "ReadableStream");
  finishClass(ReadableStreamDefaultController, "ReadableStreamDefaultController");
  finishClass(ReadableStreamDefaultReader, "ReadableStreamDefaultReader");
  finishClass(ReadableStreamBYOBReader, "ReadableStreamBYOBReader");


  // =====================================================================================
  // Public classes
  // =====================================================================================

  class WritableStream {
    constructor(underlyingSink = undefined, strategy = undefined) {
      const stream = newWritableRecord();
      stream.wrapper = this;
      records.set(this, stream);
      const sink = toDict(underlyingSink, "underlyingSink");
      if (sink.type !== undefined) throw new RangeError("Invalid type is specified");
      const strategyDict = toDict(strategy, "strategy");
      const sizeAlgorithm = extractSizeAlgorithm(strategyDict);
      const highWaterMark = extractHighWaterMark(strategyDict, 1);
      const controller = { kind: "wdc" };
      const wrapper = construct(WritableStreamDefaultController, controller);
      setUpWritableController(stream, controller, sinkAlgorithms(sink, wrapper), highWaterMark, sizeAlgorithm);
    }

    get locked() {
      return brand(this, "ws", "WritableStream").writer !== undefined;
    }

    abort(reason = undefined) {
      return guard(() => {
        const stream = brand(this, "ws", "WritableStream");
        if (stream.writer !== undefined) return rejected(new TypeError("Cannot abort a stream that already has a writer"));
        return writableStreamAbort(stream, reason);
      });
    }

    close() {
      return guard(() => {
        const stream = brand(this, "ws", "WritableStream");
        if (stream.writer !== undefined) return rejected(new TypeError("Cannot close a stream that already has a writer"));
        if (writableStreamCloseQueuedOrInFlight(stream)) return rejected(new TypeError("Cannot close an already-closing stream"));
        return writableStreamClose(stream);
      });
    }

    getWriter() {
      const stream = brand(this, "ws", "WritableStream");
      const writer = newWriterRecord();
      const wrapper = construct(WritableStreamDefaultWriter, writer);
      setUpWriter(writer, stream);
      return wrapper;
    }
  }

  class WritableStreamDefaultController {
    constructor() {
      assertConstructible();
    }

    get signal() {
      return brand(this, "wdc", "WritableStreamDefaultController").abortController.signal;
    }

    error(reason = undefined) {
      const controller = brand(this, "wdc", "WritableStreamDefaultController");
      if (controller.stream.state === "writable") writableControllerError(controller, reason);
    }
  }

  class WritableStreamDefaultWriter {
    constructor(stream) {
      if (allowConstruct) return;
      const target = brand(stream, "ws", "WritableStream");
      const writer = newWriterRecord();
      records.set(this, writer);
      writer.wrapper = this;
      setUpWriter(writer, target);
    }

    get closed() {
      return guard(() => brand(this, "writer", "WritableStreamDefaultWriter").closed.promise);
    }

    get desiredSize() {
      const writer = brand(this, "writer", "WritableStreamDefaultWriter");
      if (writer.stream === undefined) throw new TypeError("Writer was released");
      return writerGetDesiredSize(writer);
    }

    get ready() {
      return guard(() => brand(this, "writer", "WritableStreamDefaultWriter").ready.promise);
    }

    abort(reason = undefined) {
      return guard(() => {
        const writer = brand(this, "writer", "WritableStreamDefaultWriter");
        if (writer.stream === undefined) return rejected(new TypeError("Writer was released"));
        return writableStreamAbort(writer.stream, reason);
      });
    }

    close() {
      return guard(() => {
        const writer = brand(this, "writer", "WritableStreamDefaultWriter");
        const stream = writer.stream;
        if (stream === undefined) return rejected(new TypeError("Writer was released"));
        if (writableStreamCloseQueuedOrInFlight(stream)) return rejected(new TypeError("Cannot close an already-closing stream"));
        return writableStreamClose(stream);
      });
    }

    releaseLock() {
      const writer = brand(this, "writer", "WritableStreamDefaultWriter");
      if (writer.stream !== undefined) writerRelease(writer);
    }

    write(chunk = undefined) {
      return guard(() => {
        const writer = brand(this, "writer", "WritableStreamDefaultWriter");
        if (writer.stream === undefined) return rejected(new TypeError("Writer was released"));
        return writerWrite(writer, chunk);
      });
    }
  }

  finishClass(WritableStream, "WritableStream");
  finishClass(WritableStreamDefaultController, "WritableStreamDefaultController");
  finishClass(WritableStreamDefaultWriter, "WritableStreamDefaultWriter");

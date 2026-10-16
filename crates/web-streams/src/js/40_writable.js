
  // =====================================================================================
  // WritableStream: stream-level abstract operations
  // =====================================================================================

  const CLOSE_SENTINEL = Symbol("close sentinel");

  function newWritableRecord() {
    return {
      kind: "ws",
      state: "writable",
      storedError: undefined,
      writer: undefined,
      controller: undefined,
      inFlightWriteRequest: undefined,
      closeRequest: undefined,
      inFlightCloseRequest: undefined,
      pendingAbortRequest: undefined,
      backpressure: false,
      writeRequests: [],
    };
  }

  const writableStreamCloseQueuedOrInFlight = (stream) =>
    stream.closeRequest !== undefined || stream.inFlightCloseRequest !== undefined;

  const writableStreamHasOperationMarkedInFlight = (stream) =>
    stream.inFlightWriteRequest !== undefined || stream.inFlightCloseRequest !== undefined;

  function writableStreamAbort(stream, reason) {
    if (stream.state === "closed" || stream.state === "errored") return resolved(undefined);
    stream.controller.abortController.abort(reason);
    if (stream.state === "closed" || stream.state === "errored") return resolved(undefined);
    if (stream.pendingAbortRequest !== undefined) return stream.pendingAbortRequest.promise.promise;
    const wasAlreadyErroring = stream.state === "erroring";
    if (wasAlreadyErroring) reason = undefined;
    const promise = defer();
    stream.pendingAbortRequest = { promise, reason, wasAlreadyErroring };
    if (!wasAlreadyErroring) writableStreamStartErroring(stream, reason);
    return promise.promise;
  }

  function writableStreamClose(stream) {
    if (stream.state === "closed" || stream.state === "errored") {
      return rejected(new TypeError("The stream is not in a state that permits close"));
    }
    const promise = defer();
    stream.closeRequest = promise;
    const writer = stream.writer;
    if (writer !== undefined && stream.backpressure && stream.state === "writable") writer.ready.resolve(undefined);
    writableControllerClose(stream.controller);
    return promise.promise;
  }

  function writableStreamAddWriteRequest(stream) {
    const promise = defer();
    stream.writeRequests.push(promise);
    return promise.promise;
  }

  function writableStreamDealWithRejection(stream, error) {
    if (stream.state === "writable") {
      writableStreamStartErroring(stream, error);
      return;
    }
    writableStreamFinishErroring(stream);
  }

  function writableStreamStartErroring(stream, reason) {
    const controller = stream.controller;
    stream.state = "erroring";
    stream.storedError = reason;
    if (stream.writer !== undefined) writerEnsureReadyPromiseRejected(stream.writer, reason);
    if (!writableStreamHasOperationMarkedInFlight(stream) && controller.started) writableStreamFinishErroring(stream);
  }

  function writableStreamFinishErroring(stream) {
    stream.state = "errored";
    writableControllerErrorSteps(stream.controller);
    const storedError = stream.storedError;
    for (const request of stream.writeRequests) request.reject(storedError);
    stream.writeRequests = [];
    if (stream.pendingAbortRequest === undefined) {
      writableStreamRejectCloseAndClosedPromiseIfNeeded(stream);
      return;
    }
    const abortRequest = stream.pendingAbortRequest;
    stream.pendingAbortRequest = undefined;
    if (abortRequest.wasAlreadyErroring) {
      abortRequest.promise.reject(storedError);
      writableStreamRejectCloseAndClosedPromiseIfNeeded(stream);
      return;
    }
    upon(
      writableControllerAbortSteps(stream.controller, abortRequest.reason),
      () => {
        abortRequest.promise.resolve(undefined);
        writableStreamRejectCloseAndClosedPromiseIfNeeded(stream);
      },
      (error) => {
        abortRequest.promise.reject(error);
        writableStreamRejectCloseAndClosedPromiseIfNeeded(stream);
      },
    );
  }

  function writableStreamFinishInFlightWrite(stream) {
    stream.inFlightWriteRequest.resolve(undefined);
    stream.inFlightWriteRequest = undefined;
  }

  function writableStreamFinishInFlightWriteWithError(stream, error) {
    stream.inFlightWriteRequest.reject(error);
    stream.inFlightWriteRequest = undefined;
    writableStreamDealWithRejection(stream, error);
  }

  function writableStreamFinishInFlightClose(stream) {
    stream.inFlightCloseRequest.resolve(undefined);
    stream.inFlightCloseRequest = undefined;
    if (stream.state === "erroring") {
      stream.storedError = undefined;
      if (stream.pendingAbortRequest !== undefined) {
        stream.pendingAbortRequest.promise.resolve(undefined);
        stream.pendingAbortRequest = undefined;
      }
    }
    stream.state = "closed";
    if (stream.writer !== undefined) stream.writer.closed.resolve(undefined);
  }

  function writableStreamFinishInFlightCloseWithError(stream, error) {
    stream.inFlightCloseRequest.reject(error);
    stream.inFlightCloseRequest = undefined;
    if (stream.pendingAbortRequest !== undefined) {
      stream.pendingAbortRequest.promise.reject(error);
      stream.pendingAbortRequest = undefined;
    }
    writableStreamDealWithRejection(stream, error);
  }

  function writableStreamMarkCloseRequestInFlight(stream) {
    stream.inFlightCloseRequest = stream.closeRequest;
    stream.closeRequest = undefined;
  }

  function writableStreamMarkFirstWriteRequestInFlight(stream) {
    stream.inFlightWriteRequest = stream.writeRequests.shift();
  }

  function writableStreamRejectCloseAndClosedPromiseIfNeeded(stream) {
    if (stream.closeRequest !== undefined) {
      stream.closeRequest.reject(stream.storedError);
      stream.closeRequest = undefined;
    }
    const writer = stream.writer;
    if (writer !== undefined) {
      writer.closed.reject(stream.storedError);
      markHandled(writer.closed.promise);
    }
  }

  function writableStreamUpdateBackpressure(stream, backpressure) {
    const writer = stream.writer;
    if (writer !== undefined && backpressure !== stream.backpressure) {
      if (backpressure) writer.ready = defer();
      else writer.ready.resolve(undefined);
    }
    stream.backpressure = backpressure;
  }

  // ---- writer ----

  function newWriterRecord() {
    return { kind: "writer", stream: undefined, closed: undefined, ready: undefined };
  }

  function setUpWriter(writer, stream) {
    if (stream.writer !== undefined) throw new TypeError("This stream has already been locked for exclusive writing");
    writer.stream = stream;
    stream.writer = writer;
    const { state } = stream;
    if (state === "writable") {
      const needsWait = !writableStreamCloseQueuedOrInFlight(stream) && stream.backpressure;
      writer.ready = needsWait ? defer() : deferResolved(undefined);
      writer.closed = defer();
    } else if (state === "erroring") {
      writer.ready = deferRejected(stream.storedError);
      writer.closed = defer();
    } else if (state === "closed") {
      writer.ready = deferResolved(undefined);
      writer.closed = deferResolved(undefined);
    } else {
      writer.ready = deferRejected(stream.storedError);
      writer.closed = deferRejected(stream.storedError);
    }
  }

  function acquireWriterRecord(stream) {
    const writer = newWriterRecord();
    setUpWriter(writer, stream);
    return writer;
  }

  function writerCloseWithErrorPropagation(writer) {
    const stream = writer.stream;
    if (writableStreamCloseQueuedOrInFlight(stream) || stream.state === "closed") return resolved(undefined);
    if (stream.state === "errored") return rejected(stream.storedError);
    return writableStreamClose(stream);
  }

  function writerEnsureClosedPromiseRejected(writer, error) {
    if (writer.closed.pending) writer.closed.reject(error);
    else writer.closed = deferRejected(error);
    markHandled(writer.closed.promise);
  }

  function writerEnsureReadyPromiseRejected(writer, error) {
    if (writer.ready.pending) writer.ready.reject(error);
    else writer.ready = deferRejected(error);
    markHandled(writer.ready.promise);
  }

  function writerGetDesiredSize(writer) {
    const { state } = writer.stream;
    if (state === "errored" || state === "erroring") return null;
    if (state === "closed") return 0;
    return writableControllerGetDesiredSize(writer.stream.controller);
  }

  function writerRelease(writer) {
    const stream = writer.stream;
    const released = new TypeError("Writer was released");
    writerEnsureReadyPromiseRejected(writer, released);
    writerEnsureClosedPromiseRejected(writer, released);
    stream.writer = undefined;
    writer.stream = undefined;
  }

  function writerWrite(writer, chunk) {
    const stream = writer.stream;
    const controller = stream.controller;
    const chunkSize = writableControllerGetChunkSize(controller, chunk);
    if (stream !== writer.stream) return rejected(new TypeError("Writer was released"));
    const { state } = stream;
    if (state === "errored") return rejected(stream.storedError);
    if (writableStreamCloseQueuedOrInFlight(stream) || state === "closed") {
      return rejected(new TypeError("The stream is closing or closed and cannot be written to"));
    }
    if (state === "erroring") return rejected(stream.storedError);
    const promise = writableStreamAddWriteRequest(stream);
    writableControllerWrite(controller, chunk, chunkSize);
    return promise;
  }

  // =====================================================================================
  // WritableStreamDefaultController
  // =====================================================================================

  function writableControllerGetDesiredSize(controller) {
    return controller.strategyHWM - controller.queueTotalSize;
  }

  const writableControllerGetBackpressure = (controller) => writableControllerGetDesiredSize(controller) <= 0;

  function writableControllerClearAlgorithms(controller) {
    controller.writeAlgorithm = undefined;
    controller.closeAlgorithm = undefined;
    controller.abortAlgorithm = undefined;
    controller.strategySize = undefined;
  }

  function writableControllerError(controller, error) {
    writableControllerClearAlgorithms(controller);
    writableStreamStartErroring(controller.stream, error);
  }

  function writableControllerErrorIfNeeded(controller, error) {
    if (controller.stream.state === "writable") writableControllerError(controller, error);
  }

  function writableControllerErrorSteps(controller) {
    resetQueue(controller);
  }

  function writableControllerAbortSteps(controller, reason) {
    const result = controller.abortAlgorithm(reason);
    writableControllerClearAlgorithms(controller);
    return result;
  }

  function writableControllerClose(controller) {
    enqueueValueWithSize(controller, CLOSE_SENTINEL, 0);
    writableControllerAdvanceQueueIfNeeded(controller);
  }

  function writableControllerGetChunkSize(controller, chunk) {
    if (controller.strategySize === undefined) return 1;
    try {
      return controller.strategySize(chunk);
    } catch (error) {
      writableControllerErrorIfNeeded(controller, error);
      return 1;
    }
  }

  function writableControllerWrite(controller, chunk, chunkSize) {
    try {
      enqueueValueWithSize(controller, chunk, chunkSize);
    } catch (error) {
      writableControllerErrorIfNeeded(controller, error);
      return;
    }
    const stream = controller.stream;
    if (!writableStreamCloseQueuedOrInFlight(stream) && stream.state === "writable") {
      writableStreamUpdateBackpressure(stream, writableControllerGetBackpressure(controller));
    }
    writableControllerAdvanceQueueIfNeeded(controller);
  }

  function writableControllerAdvanceQueueIfNeeded(controller) {
    const stream = controller.stream;
    if (!controller.started || stream.inFlightWriteRequest !== undefined) return;
    if (stream.state === "closed" || stream.state === "errored") return;
    if (stream.state === "erroring") {
      writableStreamFinishErroring(stream);
      return;
    }
    if (controller.queue.length === 0) return;
    const value = peekQueueValue(controller);
    if (value === CLOSE_SENTINEL) writableControllerProcessClose(controller);
    else writableControllerProcessWrite(controller, value);
  }

  function writableControllerProcessClose(controller) {
    const stream = controller.stream;
    writableStreamMarkCloseRequestInFlight(stream);
    dequeueValueWithSize(controller);
    const sinkClosePromise = controller.closeAlgorithm();
    writableControllerClearAlgorithms(controller);
    upon(
      sinkClosePromise,
      () => writableStreamFinishInFlightClose(stream),
      (error) => writableStreamFinishInFlightCloseWithError(stream, error),
    );
  }

  function writableControllerProcessWrite(controller, chunk) {
    const stream = controller.stream;
    writableStreamMarkFirstWriteRequestInFlight(stream);
    upon(
      controller.writeAlgorithm(chunk),
      () => {
        writableStreamFinishInFlightWrite(stream);
        const { state } = stream;
        dequeueValueWithSize(controller);
        if (!writableStreamCloseQueuedOrInFlight(stream) && state === "writable") {
          writableStreamUpdateBackpressure(stream, writableControllerGetBackpressure(controller));
        }
        writableControllerAdvanceQueueIfNeeded(controller);
      },
      (error) => {
        if (stream.state === "writable") writableControllerClearAlgorithms(controller);
        writableStreamFinishInFlightWriteWithError(stream, error);
      },
    );
  }

  function setUpWritableController(stream, controller, algorithms, highWaterMark, sizeAlgorithm) {
    Object.assign(controller, {
      stream,
      started: false,
      strategySize: sizeAlgorithm,
      strategyHWM: highWaterMark,
      writeAlgorithm: algorithms.write,
      closeAlgorithm: algorithms.close,
      abortAlgorithm: algorithms.abort,
      abortController: new global.AbortController(),
    });
    resetQueue(controller);
    stream.controller = controller;
    writableStreamUpdateBackpressure(stream, writableControllerGetBackpressure(controller));
    const startResult = algorithms.start();
    upon(
      Promise.resolve(startResult),
      () => {
        controller.started = true;
        writableControllerAdvanceQueueIfNeeded(controller);
      },
      (error) => {
        controller.started = true;
        writableStreamDealWithRejection(stream, error);
      },
    );
  }

  function sinkAlgorithms(sink, wrapper) {
    const start = optionalCallback(sink, "start", "underlyingSink");
    const write = optionalCallback(sink, "write", "underlyingSink");
    const close = optionalCallback(sink, "close", "underlyingSink");
    const abort = optionalCallback(sink, "abort", "underlyingSink");
    return {
      start: start ? () => reflectApply(start, sink, [wrapper]) : () => undefined,
      write: write ? (chunk) => promiseCall(write, sink, [chunk, wrapper]) : () => resolved(undefined),
      close: close ? () => promiseCall(close, sink, []) : () => resolved(undefined),
      abort: abort ? (reason) => promiseCall(abort, sink, [reason]) : () => resolved(undefined),
    };
  }

  // Internal factory used by TransformStream and native sinks.
  function createWritableStream(algorithms, highWaterMark = 1, sizeAlgorithm = () => 1) {
    const stream = newWritableRecord();
    const controller = { kind: "wdc" };
    construct(WritableStreamDefaultController, controller);
    setUpWritableController(stream, controller, algorithms, highWaterMark, sizeAlgorithm);
    const wrapper = Object.create(WritableStream.prototype);
    stream.wrapper = wrapper;
    records.set(wrapper, stream);
    return wrapper;
  }

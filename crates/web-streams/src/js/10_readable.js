
  // =====================================================================================
  // ReadableStream: stream-level abstract operations
  // =====================================================================================

  const isLocked = (stream) => stream.reader !== undefined;

  function newReadableRecord() {
    return {
      kind: "rs",
      state: "readable",
      reader: undefined,
      storedError: undefined,
      disturbed: false,
      controller: undefined,
    };
  }

  function readableStreamCancel(stream, reason) {
    stream.disturbed = true;
    if (stream.state === "closed") return resolved(undefined);
    if (stream.state === "errored") return rejected(stream.storedError);
    readableStreamClose(stream);
    const reader = stream.reader;
    if (reader !== undefined && reader.kind === "byob-reader") {
      const requests = reader.readIntoRequests;
      reader.readIntoRequests = [];
      for (const request of requests) request.close(undefined);
    }
    return upon(controllerCancelSteps(stream.controller, reason), () => undefined);
  }

  function readableStreamClose(stream) {
    stream.state = "closed";
    const reader = stream.reader;
    if (reader === undefined) return;
    reader.closed.resolve(undefined);
    if (reader.kind !== "default-reader") return;
    const requests = reader.readRequests;
    reader.readRequests = [];
    for (const request of requests) request.close();
  }

  function readableStreamError(stream, error) {
    stream.state = "errored";
    stream.storedError = error;
    const reader = stream.reader;
    if (reader === undefined) return;
    reader.closed.reject(error);
    markHandled(reader.closed.promise);
    if (reader.kind === "default-reader") {
      readerErrorReadRequests(reader, error);
    } else {
      readerErrorReadIntoRequests(reader, error);
    }
  }

  function readableStreamFulfillReadRequest(stream, chunk, done) {
    const request = stream.reader.readRequests.shift();
    if (done) request.close();
    else request.chunk(chunk);
  }

  function readableStreamFulfillReadIntoRequest(stream, chunk, done) {
    const request = stream.reader.readIntoRequests.shift();
    if (done) request.close(chunk);
    else request.chunk(chunk);
  }

  const hasDefaultReader = (stream) => stream.reader?.kind === "default-reader";
  const hasByobReader = (stream) => stream.reader?.kind === "byob-reader";
  const numReadRequests = (stream) => (hasDefaultReader(stream) ? stream.reader.readRequests.length : 0);
  const numReadIntoRequests = (stream) => (hasByobReader(stream) ? stream.reader.readIntoRequests.length : 0);

  // ---- readers ----

  function newReader(kind) {
    const reader = { kind, stream: undefined, closed: defer() };
    if (kind === "default-reader") reader.readRequests = [];
    else reader.readIntoRequests = [];
    return reader;
  }

  function setUpReader(reader, stream) {
    if (isLocked(stream)) throw new TypeError("This stream has already been locked for exclusive reading");
    reader.stream = stream;
    stream.reader = reader;
    if (stream.state === "readable") {
      reader.closed = defer();
    } else if (stream.state === "closed") {
      reader.closed = deferResolved(undefined);
    } else {
      reader.closed = deferRejected(stream.storedError);
    }
  }

  function readerCancel(reader, reason) {
    return readableStreamCancel(reader.stream, reason);
  }

  function readerGenericRelease(reader) {
    const stream = reader.stream;
    const released = new TypeError("Reader was released");
    if (stream.state === "readable") {
      reader.closed.reject(released);
    } else {
      reader.closed = deferRejected(released);
    }
    markHandled(reader.closed.promise);
    controllerReleaseSteps(stream.controller);
    stream.reader = undefined;
    reader.stream = undefined;
  }

  function readerErrorReadRequests(reader, error) {
    const requests = reader.readRequests;
    reader.readRequests = [];
    for (const request of requests) request.error(error);
  }

  function readerErrorReadIntoRequests(reader, error) {
    const requests = reader.readIntoRequests;
    reader.readIntoRequests = [];
    for (const request of requests) request.error(error);
  }

  function defaultReaderRelease(reader) {
    readerGenericRelease(reader);
    readerErrorReadRequests(reader, new TypeError("Reader was released"));
  }

  function byobReaderRelease(reader) {
    readerGenericRelease(reader);
    readerErrorReadIntoRequests(reader, new TypeError("Reader was released"));
  }

  function defaultReaderRead(reader, request) {
    const stream = reader.stream;
    stream.disturbed = true;
    if (stream.state === "closed") request.close();
    else if (stream.state === "errored") request.error(stream.storedError);
    else controllerPullSteps(stream.controller, request);
  }

  function byobReaderRead(reader, view, min, request) {
    const stream = reader.stream;
    stream.disturbed = true;
    if (stream.state === "errored") request.error(stream.storedError);
    else byteControllerPullInto(stream.controller, view, min, request);
  }

  // The stream's controller is either a default or a byte controller.
  function controllerCancelSteps(controller, reason) {
    return controller.kind === "rdc" ? defaultControllerCancelSteps(controller, reason) : byteControllerCancelSteps(controller, reason);
  }

  function controllerPullSteps(controller, request) {
    if (controller.kind === "rdc") defaultControllerPullSteps(controller, request);
    else byteControllerPullSteps(controller, request);
  }

  function controllerReleaseSteps(controller) {
    if (controller.kind === "brc") byteControllerReleaseSteps(controller);
  }

  // =====================================================================================
  // ReadableStreamDefaultController
  // =====================================================================================

  function defaultControllerCanCloseOrEnqueue(controller) {
    return !controller.closeRequested && controller.stream.state === "readable";
  }

  function defaultControllerGetDesiredSize(controller) {
    const state = controller.stream.state;
    if (state === "errored") return null;
    if (state === "closed") return 0;
    return controller.strategyHWM - controller.queueTotalSize;
  }

  function defaultControllerShouldCallPull(controller) {
    if (!defaultControllerCanCloseOrEnqueue(controller) || !controller.started) return false;
    const stream = controller.stream;
    if (isLocked(stream) && numReadRequests(stream) > 0) return true;
    return defaultControllerGetDesiredSize(controller) > 0;
  }

  function defaultControllerHasBackpressure(controller) {
    return !defaultControllerShouldCallPull(controller);
  }

  function defaultControllerCallPullIfNeeded(controller) {
    if (!defaultControllerShouldCallPull(controller)) return;
    if (controller.pulling) {
      controller.pullAgain = true;
      return;
    }
    controller.pulling = true;
    upon(
      controller.pullAlgorithm(),
      () => {
        controller.pulling = false;
        if (controller.pullAgain) {
          controller.pullAgain = false;
          defaultControllerCallPullIfNeeded(controller);
        }
      },
      (error) => defaultControllerError(controller, error),
    );
  }

  function defaultControllerClearAlgorithms(controller) {
    controller.pullAlgorithm = undefined;
    controller.cancelAlgorithm = undefined;
    controller.strategySize = undefined;
  }

  function defaultControllerClose(controller) {
    if (!defaultControllerCanCloseOrEnqueue(controller)) return;
    controller.closeRequested = true;
    if (controller.queue.length === 0) {
      defaultControllerClearAlgorithms(controller);
      readableStreamClose(controller.stream);
    }
  }

  function defaultControllerEnqueue(controller, chunk) {
    if (!defaultControllerCanCloseOrEnqueue(controller)) return;
    const stream = controller.stream;
    if (isLocked(stream) && numReadRequests(stream) > 0) {
      readableStreamFulfillReadRequest(stream, chunk, false);
    } else {
      try {
        enqueueValueWithSize(controller, chunk, controller.strategySize(chunk));
      } catch (error) {
        defaultControllerError(controller, error);
        throw error;
      }
    }
    defaultControllerCallPullIfNeeded(controller);
  }

  function defaultControllerError(controller, error) {
    const stream = controller.stream;
    if (stream.state !== "readable") return;
    resetQueue(controller);
    defaultControllerClearAlgorithms(controller);
    readableStreamError(stream, error);
  }

  function defaultControllerCancelSteps(controller, reason) {
    resetQueue(controller);
    const result = controller.cancelAlgorithm(reason);
    defaultControllerClearAlgorithms(controller);
    return result;
  }

  function defaultControllerPullSteps(controller, request) {
    const stream = controller.stream;
    if (controller.queue.length > 0) {
      const chunk = dequeueValueWithSize(controller);
      if (controller.closeRequested && controller.queue.length === 0) {
        defaultControllerClearAlgorithms(controller);
        readableStreamClose(stream);
      } else {
        defaultControllerCallPullIfNeeded(controller);
      }
      request.chunk(chunk);
    } else {
      stream.reader.readRequests.push(request);
      defaultControllerCallPullIfNeeded(controller);
    }
  }

  function setUpDefaultController(stream, controller, algorithms, highWaterMark, sizeAlgorithm) {
    Object.assign(controller, {
      stream,
      started: false,
      closeRequested: false,
      pullAgain: false,
      pulling: false,
      strategySize: sizeAlgorithm,
      strategyHWM: highWaterMark,
      pullAlgorithm: algorithms.pull,
      cancelAlgorithm: algorithms.cancel,
    });
    resetQueue(controller);
    stream.controller = controller;
    const startResult = algorithms.start();
    upon(
      Promise.resolve(startResult),
      () => {
        controller.started = true;
        defaultControllerCallPullIfNeeded(controller);
      },
      (error) => defaultControllerError(controller, error),
    );
  }

  // Builds the algorithms from an underlying-source object; `wrapper` is the public controller.
  function sourceAlgorithms(source, wrapper) {
    const start = optionalCallback(source, "start", "underlyingSource");
    const pull = optionalCallback(source, "pull", "underlyingSource");
    const cancel = optionalCallback(source, "cancel", "underlyingSource");
    return {
      start: start ? () => reflectApply(start, source, [wrapper]) : () => undefined,
      pull: pull ? () => promiseCall(pull, source, [wrapper]) : () => resolved(undefined),
      cancel: cancel ? (reason) => promiseCall(cancel, source, [reason]) : () => resolved(undefined),
    };
  }

  function setUpDefaultControllerFromSource(stream, source, highWaterMark, sizeAlgorithm) {
    const controller = { kind: "rdc" };
    const wrapper = construct(ReadableStreamDefaultController, controller);
    setUpDefaultController(stream, controller, sourceAlgorithms(source, wrapper), highWaterMark, sizeAlgorithm);
  }

  // Internal factory used by tee, from(), TransformStream and native sources.
  function createReadableStream(algorithms, highWaterMark = 1, sizeAlgorithm = () => 1) {
    const stream = newReadableRecord();
    const controller = { kind: "rdc" };
    construct(ReadableStreamDefaultController, controller);
    setUpDefaultController(stream, controller, algorithms, highWaterMark, sizeAlgorithm);
    return wrapReadable(stream);
  }

  function createReadableByteStream(algorithms) {
    const stream = newReadableRecord();
    const controller = { kind: "brc" };
    construct(ReadableByteStreamController, controller);
    setUpByteController(stream, controller, algorithms, 0, undefined);
    return wrapReadable(stream);
  }

  function wrapReadable(record) {
    const wrapper = Object.create(ReadableStream.prototype);
    record.wrapper = wrapper;
    records.set(wrapper, record);
    return wrapper;
  }

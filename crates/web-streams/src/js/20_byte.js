
  // =====================================================================================
  // ReadableByteStreamController and BYOB support
  // =====================================================================================

  function byteControllerGetDesiredSize(controller) {
    const state = controller.stream.state;
    if (state === "errored") return null;
    if (state === "closed") return 0;
    return controller.strategyHWM - controller.queueTotalSize;
  }

  function byteControllerShouldCallPull(controller) {
    const stream = controller.stream;
    if (stream.state !== "readable" || controller.closeRequested || !controller.started) return false;
    if (hasDefaultReader(stream) && numReadRequests(stream) > 0) return true;
    if (hasByobReader(stream) && numReadIntoRequests(stream) > 0) return true;
    return byteControllerGetDesiredSize(controller) > 0;
  }

  function byteControllerCallPullIfNeeded(controller) {
    if (!byteControllerShouldCallPull(controller)) return;
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
          byteControllerCallPullIfNeeded(controller);
        }
      },
      (error) => byteControllerError(controller, error),
    );
  }

  function byteControllerClearAlgorithms(controller) {
    controller.pullAlgorithm = undefined;
    controller.cancelAlgorithm = undefined;
  }

  function byteControllerInvalidateByobRequest(controller) {
    const request = controller.byobRequest;
    if (request === null) return;
    request.controller = undefined;
    request.view = null;
    controller.byobRequest = null;
  }

  function byteControllerClearPendingPullIntos(controller) {
    byteControllerInvalidateByobRequest(controller);
    controller.pendingPullIntos = [];
  }

  function byteControllerShiftPendingPullInto(controller) {
    return controller.pendingPullIntos.shift();
  }

  function byteControllerError(controller, error) {
    const stream = controller.stream;
    if (stream.state !== "readable") return;
    byteControllerClearPendingPullIntos(controller);
    resetQueue(controller);
    byteControllerClearAlgorithms(controller);
    readableStreamError(stream, error);
  }

  function byteControllerClose(controller) {
    const stream = controller.stream;
    if (controller.closeRequested || stream.state !== "readable") return;
    if (controller.queueTotalSize > 0) {
      controller.closeRequested = true;
      return;
    }
    if (controller.pendingPullIntos.length > 0) {
      const first = controller.pendingPullIntos[0];
      if (first.bytesFilled % first.elementSize !== 0) {
        const error = new TypeError("Insufficient bytes to fill elements in the given buffer");
        byteControllerError(controller, error);
        throw error;
      }
    }
    byteControllerClearAlgorithms(controller);
    readableStreamClose(stream);
  }

  function byteControllerEnqueueChunkToQueue(controller, buffer, byteOffset, byteLength) {
    controller.queue.push({ buffer, byteOffset, byteLength });
    controller.queueTotalSize += byteLength;
  }

  function byteControllerEnqueueClonedChunkToQueue(controller, buffer, byteOffset, byteLength) {
    let clone;
    try {
      clone = buffer.slice(byteOffset, byteOffset + byteLength);
    } catch (error) {
      byteControllerError(controller, error);
      throw error;
    }
    byteControllerEnqueueChunkToQueue(controller, clone, 0, byteLength);
  }

  function byteControllerEnqueueDetachedPullIntoToQueue(controller, descriptor) {
    if (descriptor.bytesFilled > 0) {
      byteControllerEnqueueClonedChunkToQueue(controller, descriptor.buffer, descriptor.byteOffset, descriptor.bytesFilled);
    }
    byteControllerShiftPendingPullInto(controller);
  }

  function byteControllerEnqueue(controller, chunk) {
    const stream = controller.stream;
    if (controller.closeRequested || stream.state !== "readable") return;
    const { buffer, byteOffset, byteLength } = chunk;
    if (isDetached(buffer)) throw new TypeError("The chunk's buffer is detached");
    const transferred = transferBuffer(buffer);
    if (controller.pendingPullIntos.length > 0) {
      const first = controller.pendingPullIntos[0];
      if (isDetached(first.buffer)) throw new TypeError("The BYOB request's buffer has been detached");
      byteControllerInvalidateByobRequest(controller);
      first.buffer = transferBuffer(first.buffer);
      if (first.readerType === "none") byteControllerEnqueueDetachedPullIntoToQueue(controller, first);
    }
    if (hasDefaultReader(stream)) {
      byteControllerProcessReadRequestsUsingQueue(controller);
      if (numReadRequests(stream) === 0) {
        byteControllerEnqueueChunkToQueue(controller, transferred, byteOffset, byteLength);
      } else {
        if (controller.pendingPullIntos.length > 0) byteControllerShiftPendingPullInto(controller);
        readableStreamFulfillReadRequest(stream, new Uint8Array(transferred, byteOffset, byteLength), false);
      }
    } else if (hasByobReader(stream)) {
      byteControllerEnqueueChunkToQueue(controller, transferred, byteOffset, byteLength);
      byteControllerProcessPullIntoDescriptorsUsingQueue(controller);
    } else {
      byteControllerEnqueueChunkToQueue(controller, transferred, byteOffset, byteLength);
    }
    byteControllerCallPullIfNeeded(controller);
  }

  function byteControllerFillPullIntoDescriptorFromQueue(controller, descriptor) {
    const maxBytesToCopy = Math.min(controller.queueTotalSize, descriptor.byteLength - descriptor.bytesFilled);
    const maxBytesFilled = descriptor.bytesFilled + maxBytesToCopy;
    let remaining = maxBytesToCopy;
    let ready = false;
    const maxAlignedBytes = maxBytesFilled - (maxBytesFilled % descriptor.elementSize);
    if (maxAlignedBytes >= descriptor.minimumFill) {
      remaining = maxAlignedBytes - descriptor.bytesFilled;
      ready = true;
    }
    const queue = controller.queue;
    while (remaining > 0) {
      const head = queue[0];
      const bytesToCopy = Math.min(remaining, head.byteLength);
      copyBytes(head.buffer, head.byteOffset, descriptor.buffer, descriptor.byteOffset + descriptor.bytesFilled, bytesToCopy);
      if (head.byteLength === bytesToCopy) {
        queue.shift();
      } else {
        head.byteOffset += bytesToCopy;
        head.byteLength -= bytesToCopy;
      }
      controller.queueTotalSize -= bytesToCopy;
      descriptor.bytesFilled += bytesToCopy;
      remaining -= bytesToCopy;
    }
    return ready;
  }

  function byteControllerHandleQueueDrain(controller) {
    if (controller.queueTotalSize === 0 && controller.closeRequested) {
      byteControllerClearAlgorithms(controller);
      readableStreamClose(controller.stream);
    } else {
      byteControllerCallPullIfNeeded(controller);
    }
  }

  function byteControllerFillReadRequestFromQueue(controller, request) {
    const entry = controller.queue.shift();
    controller.queueTotalSize -= entry.byteLength;
    byteControllerHandleQueueDrain(controller);
    request.chunk(new Uint8Array(entry.buffer, entry.byteOffset, entry.byteLength));
  }

  function byteControllerProcessReadRequestsUsingQueue(controller) {
    const reader = controller.stream.reader;
    while (reader.readRequests.length > 0) {
      if (controller.queueTotalSize === 0) return;
      byteControllerFillReadRequestFromQueue(controller, reader.readRequests.shift());
    }
  }

  function convertPullIntoDescriptor(descriptor) {
    const elements = descriptor.bytesFilled / descriptor.elementSize;
    return new descriptor.viewConstructor(descriptor.buffer, descriptor.byteOffset, elements);
  }

  function commitPullIntoDescriptor(stream, descriptor) {
    const done = stream.state === "closed";
    const view = convertPullIntoDescriptor(descriptor);
    if (descriptor.readerType === "default") readableStreamFulfillReadRequest(stream, view, done);
    else readableStreamFulfillReadIntoRequest(stream, view, done);
  }

  function byteControllerProcessPullIntoDescriptorsUsingQueue(controller) {
    while (controller.pendingPullIntos.length > 0) {
      if (controller.queueTotalSize === 0) return;
      const descriptor = controller.pendingPullIntos[0];
      if (byteControllerFillPullIntoDescriptorFromQueue(controller, descriptor)) {
        byteControllerShiftPendingPullInto(controller);
        commitPullIntoDescriptor(controller.stream, descriptor);
      }
    }
  }

  function byteControllerPullInto(controller, view, min, request) {
    const stream = controller.stream;
    let elementSize = 1;
    let viewConstructor = DataView;
    if (!(view instanceof DataView)) {
      viewConstructor = view.constructor;
      elementSize = viewConstructor.BYTES_PER_ELEMENT;
    }
    const { byteOffset, byteLength } = view;
    let buffer;
    try {
      buffer = transferBuffer(view.buffer);
    } catch (error) {
      request.error(error);
      return;
    }
    const descriptor = {
      buffer,
      bufferByteLength: buffer.byteLength,
      byteOffset,
      byteLength,
      bytesFilled: 0,
      minimumFill: min * elementSize,
      elementSize,
      viewConstructor,
      readerType: "byob",
    };
    if (controller.pendingPullIntos.length > 0) {
      controller.pendingPullIntos.push(descriptor);
      stream.reader.readIntoRequests.push(request);
      return;
    }
    if (stream.state === "closed") {
      request.close(new viewConstructor(descriptor.buffer, byteOffset, 0));
      return;
    }
    if (controller.queueTotalSize > 0) {
      if (byteControllerFillPullIntoDescriptorFromQueue(controller, descriptor)) {
        const filled = convertPullIntoDescriptor(descriptor);
        byteControllerHandleQueueDrain(controller);
        request.chunk(filled);
        return;
      }
      if (controller.closeRequested) {
        const error = new TypeError("Insufficient bytes to fill elements in the given buffer");
        byteControllerError(controller, error);
        request.error(error);
        return;
      }
    }
    controller.pendingPullIntos.push(descriptor);
    stream.reader.readIntoRequests.push(request);
    byteControllerCallPullIfNeeded(controller);
  }

  function byteControllerRespondInClosedState(controller, descriptor) {
    if (descriptor.readerType === "none") byteControllerShiftPendingPullInto(controller);
    const stream = controller.stream;
    if (!hasByobReader(stream)) return;
    while (numReadIntoRequests(stream) > 0) {
      commitPullIntoDescriptor(stream, byteControllerShiftPendingPullInto(controller));
    }
  }

  function byteControllerRespondInReadableState(controller, bytesWritten, descriptor) {
    descriptor.bytesFilled += bytesWritten;
    if (descriptor.readerType === "none") {
      byteControllerEnqueueDetachedPullIntoToQueue(controller, descriptor);
      byteControllerProcessPullIntoDescriptorsUsingQueue(controller);
      return;
    }
    if (descriptor.bytesFilled < descriptor.minimumFill) return;
    byteControllerShiftPendingPullInto(controller);
    const remainder = descriptor.bytesFilled % descriptor.elementSize;
    if (remainder > 0) {
      const end = descriptor.byteOffset + descriptor.bytesFilled;
      byteControllerEnqueueClonedChunkToQueue(controller, descriptor.buffer, end - remainder, remainder);
    }
    descriptor.bytesFilled -= remainder;
    commitPullIntoDescriptor(controller.stream, descriptor);
    byteControllerProcessPullIntoDescriptorsUsingQueue(controller);
  }

  function byteControllerRespondInternal(controller, bytesWritten) {
    const descriptor = controller.pendingPullIntos[0];
    byteControllerInvalidateByobRequest(controller);
    if (controller.stream.state === "closed") byteControllerRespondInClosedState(controller, descriptor);
    else byteControllerRespondInReadableState(controller, bytesWritten, descriptor);
    byteControllerCallPullIfNeeded(controller);
  }

  function byteControllerRespond(controller, bytesWritten) {
    const descriptor = controller.pendingPullIntos[0];
    if (controller.stream.state === "closed") {
      if (bytesWritten !== 0) throw new TypeError("bytesWritten must be 0 when the stream is closed");
    } else {
      if (bytesWritten === 0) throw new TypeError("bytesWritten must be greater than 0 when the stream is readable");
      if (descriptor.bytesFilled + bytesWritten > descriptor.byteLength) throw new RangeError("bytesWritten out of range");
    }
    descriptor.buffer = transferBuffer(descriptor.buffer);
    byteControllerRespondInternal(controller, bytesWritten);
  }

  function byteControllerRespondWithNewView(controller, view) {
    const descriptor = controller.pendingPullIntos[0];
    if (controller.stream.state === "closed") {
      if (view.byteLength !== 0) throw new TypeError("The view must have zero byteLength when the stream is closed");
    } else if (view.byteLength === 0) {
      throw new TypeError("The view must have non-zero byteLength when the stream is readable");
    }
    if (descriptor.byteOffset + descriptor.bytesFilled !== view.byteOffset) throw new RangeError("The view's byteOffset does not match the request");
    if (descriptor.bufferByteLength !== view.buffer.byteLength) throw new RangeError("The view's buffer has a different capacity");
    if (descriptor.bytesFilled + view.byteLength > descriptor.byteLength) throw new RangeError("The view is larger than the request");
    const viewByteLength = view.byteLength;
    descriptor.buffer = transferBuffer(view.buffer);
    byteControllerRespondInternal(controller, viewByteLength);
  }

  function byteControllerGetByobRequest(controller) {
    if (controller.byobRequest === null && controller.pendingPullIntos.length > 0) {
      const first = controller.pendingPullIntos[0];
      const view = new Uint8Array(first.buffer, first.byteOffset + first.bytesFilled, first.byteLength - first.bytesFilled);
      const request = { kind: "byob-request", controller, view };
      construct(ReadableStreamBYOBRequest, request);
      controller.byobRequest = request;
    }
    return controller.byobRequest;
  }

  function byteControllerCancelSteps(controller, reason) {
    byteControllerClearPendingPullIntos(controller);
    resetQueue(controller);
    const result = controller.cancelAlgorithm(reason);
    byteControllerClearAlgorithms(controller);
    return result;
  }

  function byteControllerPullSteps(controller, request) {
    const stream = controller.stream;
    if (controller.queueTotalSize > 0) {
      byteControllerFillReadRequestFromQueue(controller, request);
      return;
    }
    const size = controller.autoAllocateChunkSize;
    if (size !== undefined) {
      let buffer;
      try {
        buffer = new ArrayBuffer(size);
      } catch (error) {
        request.error(error);
        return;
      }
      controller.pendingPullIntos.push({
        buffer,
        bufferByteLength: size,
        byteOffset: 0,
        byteLength: size,
        bytesFilled: 0,
        minimumFill: 1,
        elementSize: 1,
        viewConstructor: Uint8Array,
        readerType: "default",
      });
    }
    stream.reader.readRequests.push(request);
    byteControllerCallPullIfNeeded(controller);
  }

  function byteControllerReleaseSteps(controller) {
    if (controller.pendingPullIntos.length === 0) return;
    const first = controller.pendingPullIntos[0];
    first.readerType = "none";
    controller.pendingPullIntos = [first];
  }

  function setUpByteController(stream, controller, algorithms, highWaterMark, autoAllocateChunkSize) {
    Object.assign(controller, {
      stream,
      pullAgain: false,
      pulling: false,
      byobRequest: null,
      closeRequested: false,
      started: false,
      strategyHWM: highWaterMark,
      pullAlgorithm: algorithms.pull,
      cancelAlgorithm: algorithms.cancel,
      autoAllocateChunkSize,
      pendingPullIntos: [],
    });
    resetQueue(controller);
    stream.controller = controller;
    const startResult = algorithms.start();
    upon(
      Promise.resolve(startResult),
      () => {
        controller.started = true;
        byteControllerCallPullIfNeeded(controller);
      },
      (error) => byteControllerError(controller, error),
    );
  }

  function setUpByteControllerFromSource(stream, source, highWaterMark) {
    const controller = { kind: "brc" };
    const wrapper = construct(ReadableByteStreamController, controller);
    let autoAllocateChunkSize;
    if (source.autoAllocateChunkSize !== undefined) {
      autoAllocateChunkSize = Number(source.autoAllocateChunkSize);
      if (!Number.isInteger(autoAllocateChunkSize) || autoAllocateChunkSize <= 0) {
        throw new TypeError("autoAllocateChunkSize must be a positive integer");
      }
    }
    setUpByteController(stream, controller, sourceAlgorithms(source, wrapper), highWaterMark, autoAllocateChunkSize);
  }

  class ReadableByteStreamController {
    constructor() {
      assertConstructible();
    }

    get byobRequest() {
      const request = byteControllerGetByobRequest(brand(this, "brc", "ReadableByteStreamController"));
      return request === null ? null : request.wrapper;
    }

    get desiredSize() {
      return byteControllerGetDesiredSize(brand(this, "brc", "ReadableByteStreamController"));
    }

    close() {
      const controller = brand(this, "brc", "ReadableByteStreamController");
      if (controller.closeRequested) throw new TypeError("The stream has already been requested to close");
      if (controller.stream.state !== "readable") throw new TypeError("The stream is not readable");
      byteControllerClose(controller);
    }

    enqueue(chunk) {
      const controller = brand(this, "brc", "ReadableByteStreamController");
      if (!ArrayBuffer.isView(chunk)) throw new TypeError("chunk must be an ArrayBufferView");
      if (chunk.byteLength === 0) throw new TypeError("chunk must have non-zero byteLength");
      if (chunk.buffer.byteLength === 0) throw new TypeError("chunk's buffer must have non-zero byteLength");
      if (controller.closeRequested) throw new TypeError("The stream has already been requested to close");
      if (controller.stream.state !== "readable") throw new TypeError("The stream is not readable");
      byteControllerEnqueue(controller, chunk);
    }

    error(reason = undefined) {
      byteControllerError(brand(this, "brc", "ReadableByteStreamController"), reason);
    }
  }

  class ReadableStreamBYOBRequest {
    constructor() {
      assertConstructible();
    }

    get view() {
      return brand(this, "byob-request", "ReadableStreamBYOBRequest").view;
    }

    respond(bytesWritten) {
      const request = brand(this, "byob-request", "ReadableStreamBYOBRequest");
      const written = Number(bytesWritten);
      if (!Number.isInteger(written) || written < 0) throw new TypeError("bytesWritten must be a non-negative integer");
      if (request.controller === undefined) throw new TypeError("This BYOB request has been invalidated");
      if (isDetached(request.view.buffer)) throw new TypeError("The BYOB request's buffer has been detached");
      byteControllerRespond(request.controller, written);
    }

    respondWithNewView(view) {
      const request = brand(this, "byob-request", "ReadableStreamBYOBRequest");
      if (!ArrayBuffer.isView(view)) throw new TypeError("view must be an ArrayBufferView");
      if (request.controller === undefined) throw new TypeError("This BYOB request has been invalidated");
      if (isDetached(view.buffer)) throw new TypeError("The view's buffer has been detached");
      byteControllerRespondWithNewView(request.controller, view);
    }
  }

  finishClass(ReadableByteStreamController, "ReadableByteStreamController");
  finishClass(ReadableStreamBYOBRequest, "ReadableStreamBYOBRequest");


  // =====================================================================================
  // TransformStream
  // =====================================================================================

  function transformStreamSetBackpressure(stream, backpressure) {
    if (stream.backpressureChangePromise !== undefined) stream.backpressureChangePromise.resolve(undefined);
    stream.backpressureChangePromise = defer();
    stream.backpressure = backpressure;
  }

  function transformStreamUnblockWrite(stream) {
    if (stream.backpressure === true) transformStreamSetBackpressure(stream, false);
  }

  function transformStreamErrorWritableAndUnblockWrite(stream, error) {
    transformControllerClearAlgorithms(stream.controller);
    writableControllerErrorIfNeeded(records.get(stream.writable).controller, error);
    transformStreamUnblockWrite(stream);
  }

  function transformStreamError(stream, error) {
    defaultControllerError(records.get(stream.readable).controller, error);
    transformStreamErrorWritableAndUnblockWrite(stream, error);
  }

  function transformControllerClearAlgorithms(controller) {
    controller.transformAlgorithm = undefined;
    controller.flushAlgorithm = undefined;
    controller.cancelAlgorithm = undefined;
  }

  function transformControllerEnqueue(controller, chunk) {
    const stream = controller.stream;
    const readableController = records.get(stream.readable).controller;
    if (!defaultControllerCanCloseOrEnqueue(readableController)) {
      throw new TypeError("The readable side is not in a state that permits enqueue");
    }
    try {
      defaultControllerEnqueue(readableController, chunk);
    } catch (error) {
      transformStreamErrorWritableAndUnblockWrite(stream, error);
      throw records.get(stream.readable).storedError;
    }
    const backpressure = defaultControllerHasBackpressure(readableController);
    if (backpressure !== stream.backpressure) transformStreamSetBackpressure(stream, true);
  }

  function transformControllerPerformTransform(controller, chunk) {
    return upon(controller.transformAlgorithm(chunk), undefined, (error) => {
      transformStreamError(controller.stream, error);
      throw error;
    });
  }

  function transformControllerTerminate(controller) {
    const stream = controller.stream;
    defaultControllerClose(records.get(stream.readable).controller);
    transformStreamErrorWritableAndUnblockWrite(stream, new TypeError("TransformStream terminated"));
  }

  // Shared tail of the sink-abort, sink-close and source-cancel algorithms.
  function finishWith(controller, work, onSuccess, onFailure) {
    if (controller.finishPromise !== undefined) return controller.finishPromise.promise;
    const finish = defer();
    controller.finishPromise = finish;
    const result = work();
    transformControllerClearAlgorithms(controller);
    upon(
      result,
      () => onSuccess(finish),
      (error) => {
        onFailure(error);
        finish.reject(error);
      },
    );
    return finish.promise;
  }

  function transformSinkWrite(stream, chunk) {
    const controller = stream.controller;
    if (!stream.backpressure) return transformControllerPerformTransform(controller, chunk);
    return upon(stream.backpressureChangePromise.promise, () => {
      const writable = records.get(stream.writable);
      if (writable.state === "erroring") throw writable.storedError;
      return transformControllerPerformTransform(controller, chunk);
    });
  }

  function transformSinkAbort(stream, reason) {
    const controller = stream.controller;
    const readable = records.get(stream.readable);
    return finishWith(
      controller,
      () => controller.cancelAlgorithm(reason),
      (finish) => {
        if (readable.state === "errored") {
          finish.reject(readable.storedError);
        } else {
          defaultControllerError(readable.controller, reason);
          finish.resolve(undefined);
        }
      },
      (error) => defaultControllerError(readable.controller, error),
    );
  }

  function transformSinkClose(stream) {
    const controller = stream.controller;
    const readable = records.get(stream.readable);
    return finishWith(
      controller,
      () => controller.flushAlgorithm(),
      (finish) => {
        if (readable.state === "errored") {
          finish.reject(readable.storedError);
        } else {
          defaultControllerClose(readable.controller);
          finish.resolve(undefined);
        }
      },
      (error) => defaultControllerError(readable.controller, error),
    );
  }

  function transformSourceCancel(stream, reason) {
    const controller = stream.controller;
    const writable = records.get(stream.writable);
    const failWritable = (error) => {
      writableControllerErrorIfNeeded(writable.controller, error);
      transformStreamUnblockWrite(stream);
    };
    return finishWith(
      controller,
      () => controller.cancelAlgorithm(reason),
      (finish) => {
        if (writable.state === "errored") {
          finish.reject(writable.storedError);
        } else {
          failWritable(reason);
          finish.resolve(undefined);
        }
      },
      failWritable,
    );
  }

  function transformSourcePull(stream) {
    transformStreamSetBackpressure(stream, false);
    return stream.backpressureChangePromise.promise;
  }

  function transformAlgorithms(transformer, wrapper) {
    const transform = optionalCallback(transformer, "transform", "transformer");
    const flush = optionalCallback(transformer, "flush", "transformer");
    const cancel = optionalCallback(transformer, "cancel", "transformer");
    return {
      transform: transform
        ? (chunk) => promiseCall(transform, transformer, [chunk, wrapper])
        : (chunk) => {
            try {
              transformControllerEnqueue(records.get(wrapper), chunk);
            } catch (error) {
              return rejected(error);
            }
            return resolved(undefined);
          },
      flush: flush ? () => promiseCall(flush, transformer, [wrapper]) : () => resolved(undefined),
      cancel: cancel ? (reason) => promiseCall(cancel, transformer, [reason]) : () => resolved(undefined),
    };
  }

  class TransformStream {
    #record;

    constructor(transformer = undefined, writableStrategy = undefined, readableStrategy = undefined) {
      const transformerDict = toDict(transformer, "transformer");
      if (transformerDict.readableType !== undefined) throw new RangeError("Invalid readableType specified");
      if (transformerDict.writableType !== undefined) throw new RangeError("Invalid writableType specified");
      const readableDict = toDict(readableStrategy, "readableStrategy");
      const writableDict = toDict(writableStrategy, "writableStrategy");
      const readableHWM = extractHighWaterMark(readableDict, 0);
      const readableSize = extractSizeAlgorithm(readableDict);
      const writableHWM = extractHighWaterMark(writableDict, 1);
      const writableSize = extractSizeAlgorithm(writableDict);

      const stream = { backpressure: undefined, backpressureChangePromise: undefined, controller: undefined };
      this.#record = stream;
      const startPromise = defer();
      stream.writable = createWritableStream(
        {
          start: () => startPromise.promise,
          write: (chunk) => transformSinkWrite(stream, chunk),
          abort: (reason) => transformSinkAbort(stream, reason),
          close: () => transformSinkClose(stream),
        },
        writableHWM,
        writableSize,
      );
      stream.readable = createReadableStream(
        {
          start: () => startPromise.promise,
          pull: () => transformSourcePull(stream),
          cancel: (reason) => transformSourceCancel(stream, reason),
        },
        readableHWM,
        readableSize,
      );
      transformStreamSetBackpressure(stream, true);

      const controller = { kind: "tdc", stream, finishPromise: undefined };
      const wrapper = construct(TransformStreamDefaultController, controller);
      Object.assign(controller, {
        transformAlgorithm: undefined,
        flushAlgorithm: undefined,
        cancelAlgorithm: undefined,
      });
      const algorithms = transformAlgorithms(transformerDict, wrapper);
      controller.transformAlgorithm = algorithms.transform;
      controller.flushAlgorithm = algorithms.flush;
      controller.cancelAlgorithm = algorithms.cancel;
      stream.controller = controller;

      const start = optionalCallback(transformerDict, "start", "transformer");
      startPromise.resolve(start ? reflectApply(start, transformerDict, [wrapper]) : undefined);
    }

    get readable() {
      return this.#record.readable;
    }

    get writable() {
      return this.#record.writable;
    }
  }

  class TransformStreamDefaultController {
    constructor() {
      assertConstructible();
    }

    get desiredSize() {
      const controller = brand(this, "tdc", "TransformStreamDefaultController");
      return defaultControllerGetDesiredSize(records.get(controller.stream.readable).controller);
    }

    enqueue(chunk = undefined) {
      transformControllerEnqueue(brand(this, "tdc", "TransformStreamDefaultController"), chunk);
    }

    error(reason = undefined) {
      transformStreamError(brand(this, "tdc", "TransformStreamDefaultController").stream, reason);
    }

    terminate() {
      transformControllerTerminate(brand(this, "tdc", "TransformStreamDefaultController"));
    }
  }

  finishClass(TransformStream, "TransformStream");
  finishClass(TransformStreamDefaultController, "TransformStreamDefaultController");

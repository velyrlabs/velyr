
  // =====================================================================================
  // tee
  // =====================================================================================

  function readableStreamTee(stream) {
    return stream.controller.kind === "brc" ? byteStreamTee(stream) : defaultStreamTee(stream);
  }

  function defaultStreamTee(stream) {
    const reader = acquireDefaultReaderRecord(stream);
    let reading = false;
    let readAgain = false;
    const canceled = [false, false];
    const reasons = [undefined, undefined];
    const branches = [];
    const cancelPromise = defer();

    function pullAlgorithm() {
      if (reading) {
        readAgain = true;
        return resolved(undefined);
      }
      reading = true;
      defaultReaderRead(reader, {
        chunk(chunk) {
          queueMicrotask(() => {
            readAgain = false;
            if (!canceled[0]) defaultControllerEnqueue(branches[0].controller, chunk);
            if (!canceled[1]) defaultControllerEnqueue(branches[1].controller, chunk);
            reading = false;
            if (readAgain) pullAlgorithm();
          });
        },
        close() {
          reading = false;
          if (!canceled[0]) defaultControllerClose(branches[0].controller);
          if (!canceled[1]) defaultControllerClose(branches[1].controller);
          if (!canceled[0] || !canceled[1]) cancelPromise.resolve(undefined);
        },
        error() {
          reading = false;
        },
      });
      return resolved(undefined);
    }

    const cancelBranch = (index) => (reason) => {
      canceled[index] = true;
      reasons[index] = reason;
      if (canceled[1 - index]) cancelPromise.resolve(readableStreamCancel(stream, [reasons[0], reasons[1]]));
      return cancelPromise.promise;
    };

    for (const index of [0, 1]) {
      const algorithms = { start: () => undefined, pull: pullAlgorithm, cancel: cancelBranch(index) };
      branches.push(records.get(createReadableStream(algorithms)));
    }
    upon(reader.closed.promise, undefined, (error) => {
      defaultControllerError(branches[0].controller, error);
      defaultControllerError(branches[1].controller, error);
      if (!canceled[0] || !canceled[1]) cancelPromise.resolve(undefined);
    });
    return [branches[0].wrapper, branches[1].wrapper];
  }

  // A default reader record without a public wrapper, for internal consumers (tee, pipeTo).
  function acquireDefaultReaderRecord(stream) {
    const reader = newReader("default-reader");
    setUpReader(reader, stream);
    return reader;
  }

  function acquireByobReaderRecord(stream) {
    const reader = newReader("byob-reader");
    setUpReader(reader, stream);
    return reader;
  }

  function byteStreamTee(stream) {
    let reader = acquireDefaultReaderRecord(stream);
    let reading = false;
    const readAgain = [false, false];
    const canceled = [false, false];
    const reasons = [undefined, undefined];
    const branches = [];
    const cancelPromise = defer();

    function forwardReaderError(thisReader) {
      upon(thisReader.closed.promise, undefined, (error) => {
        if (thisReader !== reader) return;
        byteControllerError(branches[0].controller, error);
        byteControllerError(branches[1].controller, error);
        if (!canceled[0] || !canceled[1]) cancelPromise.resolve(undefined);
      });
    }

    function failBoth(error) {
      byteControllerError(branches[0].controller, error);
      byteControllerError(branches[1].controller, error);
      cancelPromise.resolve(readableStreamCancel(stream, error));
    }

    function pullWithDefaultReader() {
      if (reader.kind === "byob-reader") {
        byobReaderRelease(reader);
        reader = acquireDefaultReaderRecord(stream);
        forwardReaderError(reader);
      }
      defaultReaderRead(reader, {
        chunk(chunk) {
          queueMicrotask(() => {
            readAgain[0] = false;
            readAgain[1] = false;
            let chunk2 = chunk;
            if (!canceled[0] && !canceled[1]) {
              try {
                chunk2 = cloneAsUint8Array(chunk);
              } catch (error) {
                failBoth(error);
                return;
              }
            }
            if (!canceled[0]) byteControllerEnqueue(branches[0].controller, chunk);
            if (!canceled[1]) byteControllerEnqueue(branches[1].controller, chunk2);
            reading = false;
            rerunPullIfRequested();
          });
        },
        close() {
          reading = false;
          for (const index of [0, 1]) {
            if (!canceled[index]) byteControllerClose(branches[index].controller);
          }
          for (const index of [0, 1]) {
            if (branches[index].controller.pendingPullIntos.length > 0) byteControllerRespond(branches[index].controller, 0);
          }
          if (!canceled[0] || !canceled[1]) cancelPromise.resolve(undefined);
        },
        error() {
          reading = false;
        },
      });
    }

    function pullWithByobReader(view, forBranch2) {
      if (reader.kind === "default-reader") {
        defaultReaderRelease(reader);
        reader = acquireByobReaderRecord(stream);
        forwardReaderError(reader);
      }
      const byob = forBranch2 ? 1 : 0;
      const other = 1 - byob;
      byobReaderRead(reader, view, 1, {
        chunk(chunk) {
          queueMicrotask(() => {
            readAgain[0] = false;
            readAgain[1] = false;
            if (!canceled[other]) {
              let cloned;
              try {
                cloned = cloneAsUint8Array(chunk);
              } catch (error) {
                failBoth(error);
                return;
              }
              if (!canceled[byob]) byteControllerRespondWithNewView(branches[byob].controller, chunk);
              byteControllerEnqueue(branches[other].controller, cloned);
            } else if (!canceled[byob]) {
              byteControllerRespondWithNewView(branches[byob].controller, chunk);
            }
            reading = false;
            rerunPullIfRequested();
          });
        },
        close(chunk) {
          reading = false;
          if (!canceled[byob]) byteControllerClose(branches[byob].controller);
          if (!canceled[other]) byteControllerClose(branches[other].controller);
          if (chunk !== undefined) {
            if (!canceled[byob]) byteControllerRespondWithNewView(branches[byob].controller, chunk);
            if (!canceled[other] && branches[other].controller.pendingPullIntos.length > 0) {
              byteControllerRespond(branches[other].controller, 0);
            }
          }
          if (!canceled[0] || !canceled[1]) cancelPromise.resolve(undefined);
        },
        error() {
          reading = false;
        },
      });
    }

    const pullBranch = (index) => () => {
      if (reading) {
        readAgain[index] = true;
        return resolved(undefined);
      }
      reading = true;
      const request = byteControllerGetByobRequest(branches[index].controller);
      if (request === null) pullWithDefaultReader();
      else pullWithByobReader(request.view, index === 1);
      return resolved(undefined);
    };

    function rerunPullIfRequested() {
      if (readAgain[0]) pullBranch(0)();
      else if (readAgain[1]) pullBranch(1)();
    }

    const cancelBranch = (index) => (reason) => {
      canceled[index] = true;
      reasons[index] = reason;
      if (canceled[1 - index]) cancelPromise.resolve(readableStreamCancel(stream, [reasons[0], reasons[1]]));
      return cancelPromise.promise;
    };

    for (const index of [0, 1]) {
      const algorithms = { start: () => undefined, pull: pullBranch(index), cancel: cancelBranch(index) };
      branches.push(records.get(createReadableByteStream(algorithms)));
    }
    forwardReaderError(reader);
    return [branches[0].wrapper, branches[1].wrapper];
  }

  // =====================================================================================
  // pipeTo
  // =====================================================================================

  function pipeOptionsOf(options) {
    const dict = toDict(options, "options");
    const { signal } = dict;
    if (signal !== undefined && !(typeof global.AbortSignal === "function" && signal instanceof global.AbortSignal)) {
      throw new TypeError("options.signal must be an AbortSignal");
    }
    return {
      preventClose: Boolean(dict.preventClose),
      preventAbort: Boolean(dict.preventAbort),
      preventCancel: Boolean(dict.preventCancel),
      signal,
    };
  }

  function readableStreamPipeTo(source, dest, { preventClose, preventAbort, preventCancel, signal }) {
    const reader = acquireDefaultReaderRecord(source);
    const writer = acquireWriterRecord(dest);
    source.disturbed = true;
    let shuttingDown = false;
    const done = defer();
    let currentWrite = resolved(undefined);

    function waitForWritesToFinish() {
      const oldWrite = currentWrite;
      return upon(currentWrite, () => (oldWrite !== currentWrite ? waitForWritesToFinish() : undefined));
    }

    function finalize(isError, error) {
      writerRelease(writer);
      defaultReaderRelease(reader);
      if (signal !== undefined) signal.removeEventListener("abort", abortAlgorithm);
      if (isError) done.reject(error);
      else done.resolve(undefined);
    }

    const destIsOpen = () => dest.state === "writable" && !writableStreamCloseQueuedOrInFlight(dest);

    function shutdown(isError, error, action) {
      if (shuttingDown) return;
      shuttingDown = true;
      const run = () => {
        if (action === undefined) {
          finalize(isError, error);
        } else {
          upon(action(), () => finalize(isError, error), (newError) => finalize(true, newError));
        }
      };
      if (destIsOpen()) upon(waitForWritesToFinish(), run);
      else run();
    }

    function abortAlgorithm() {
      const error = signal.reason;
      const actions = [];
      if (!preventAbort) actions.push(() => (dest.state === "writable" ? writableStreamAbort(dest, error) : resolved(undefined)));
      if (!preventCancel) actions.push(() => (source.state === "readable" ? readableStreamCancel(source, error) : resolved(undefined)));
      shutdown(true, error, () => Promise.all(actions.map((action) => action())));
    }

    function onSourceError(error) {
      if (!preventAbort) shutdown(true, error, () => writableStreamAbort(dest, error));
      else shutdown(true, error);
    }

    function onDestError(error) {
      if (!preventCancel) shutdown(true, error, () => readableStreamCancel(source, error));
      else shutdown(true, error);
    }

    function onSourceClosed() {
      if (!preventClose) shutdown(false, undefined, () => writerCloseWithErrorPropagation(writer));
      else shutdown(false);
    }

    function onDestClosing() {
      const error = new TypeError("the destination writable stream closed before all data could be piped to it");
      if (!preventCancel) shutdown(true, error, () => readableStreamCancel(source, error));
      else shutdown(true, error);
    }

    if (signal !== undefined) {
      if (signal.aborted) {
        abortAlgorithm();
        return done.promise;
      }
      signal.addEventListener("abort", abortAlgorithm);
    }

    upon(reader.closed.promise, onSourceClosed, onSourceError);
    upon(writer.closed.promise, onDestClosing, onDestError);
    if (writableStreamCloseQueuedOrInFlight(dest)) onDestClosing();

    function pipeStep() {
      if (shuttingDown) return;
      upon(
        writer.ready.promise,
        () => {
          if (shuttingDown) return;
          defaultReaderRead(reader, {
            chunk(chunk) {
              currentWrite = writerWrite(writer, chunk);
              markHandled(currentWrite);
              pipeStep();
            },
            close() {},
            error() {},
          });
        },
        () => {},
      );
    }
    pipeStep();
    return done.promise;
  }

  // =====================================================================================
  // Async iteration
  // =====================================================================================

  const AsyncIteratorPrototype = Object.getPrototypeOf(Object.getPrototypeOf(async function* () {}).prototype);

  const readableStreamAsyncIteratorPrototype = Object.create(AsyncIteratorPrototype, {
    next: {
      value: function next() {
        return guard(() => {
          const state = iteratorState(this);
          const run = () => iteratorNextSteps(state);
          state.ongoing = state.ongoing ? upon(state.ongoing, run, run) : run();
          return state.ongoing;
        });
      },
      writable: true,
      configurable: true,
    },
    return: {
      value: function (value) {
        return guard(() => {
          const state = iteratorState(this);
          const run = () => iteratorReturnSteps(state, value);
          state.ongoing = state.ongoing ? upon(state.ongoing, run, run) : run();
          return state.ongoing;
        });
      },
      writable: true,
      configurable: true,
    },
  });

  const iteratorStates = new WeakMap();

  function iteratorState(iterator) {
    const state = iteratorStates.get(iterator);
    if (state === undefined) throw new TypeError("Illegal invocation: receiver is not a ReadableStream async iterator");
    return state;
  }

  function createAsyncIterator(readerWrapper, preventCancel) {
    const iterator = Object.create(readableStreamAsyncIteratorPrototype);
    iteratorStates.set(iterator, {
      reader: records.get(readerWrapper),
      preventCancel,
      ongoing: undefined,
      finished: false,
    });
    return iterator;
  }

  function iteratorNextSteps(state) {
    if (state.finished) return resolved({ value: undefined, done: true });
    const { reader } = state;
    const result = defer();
    defaultReaderRead(reader, {
      chunk: (value) => result.resolve({ value, done: false }),
      close() {
        state.finished = true;
        defaultReaderRelease(reader);
        result.resolve({ value: undefined, done: true });
      },
      error(error) {
        state.finished = true;
        defaultReaderRelease(reader);
        result.reject(error);
      },
    });
    return result.promise;
  }

  function iteratorReturnSteps(state, value) {
    if (state.finished) return resolved({ value, done: true });
    state.finished = true;
    const { reader } = state;
    if (!state.preventCancel) {
      const result = readerCancel(reader, value);
      defaultReaderRelease(reader);
      return upon(result, () => ({ value, done: true }));
    }
    defaultReaderRelease(reader);
    return resolved({ value, done: true });
  }

  // =====================================================================================
  // ReadableStream.from
  // =====================================================================================

  function readableStreamFromIterable(iterable) {
    const iterator = getAsyncIterator(iterable);
    const algorithms = {
      start: () => undefined,
      pull: () =>
        promiseCall(iterator.next, iterator, []).then((result) => {
          if (Object(result) !== result) throw new TypeError("The iterator result is not an object");
          if (result.done) defaultControllerClose(controllerOf(stream));
          else defaultControllerEnqueue(controllerOf(stream), result.value);
        }),
      cancel(reason) {
        if (iterator.return === undefined) return resolved(undefined);
        return promiseCall(iterator.return, iterator, [reason]).then((result) => {
          if (Object(result) !== result) throw new TypeError("The iterator return result is not an object");
        });
      },
    };
    const stream = createReadableStream(algorithms, 0);
    return stream;
  }

  const controllerOf = (stream) => records.get(stream).controller;

  // Returns { next, return } with promise-returning methods for async or sync iterables.
  function getAsyncIterator(iterable) {
    const asyncMethod = iterable?.[Symbol.asyncIterator];
    if (asyncMethod !== undefined && asyncMethod !== null) {
      const iterator = reflectApply(asyncMethod, iterable, []);
      if (Object(iterator) !== iterator) throw new TypeError("The async iterator is not an object");
      return { next: iterator.next.bind(iterator), return: iterator.return?.bind(iterator) };
    }
    const syncMethod = iterable?.[Symbol.iterator];
    if (typeof syncMethod !== "function") throw new TypeError("ReadableStream.from requires an iterable");
    const iterator = reflectApply(syncMethod, iterable, []);
    if (Object(iterator) !== iterator) throw new TypeError("The iterator is not an object");
    const unwrap = (result) => {
      if (Object(result) !== result) throw new TypeError("The iterator result is not an object");
      return Promise.resolve(result.value).then((value) => ({ value, done: result.done }));
    };
    return {
      next: () => promiseCall(iterator.next, iterator, []).then(unwrap),
      return: iterator.return === undefined ? undefined : (reason) => promiseCall(iterator.return, iterator, [reason]).then(unwrap),
    };
  }

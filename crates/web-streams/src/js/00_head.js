// Bootstrap: WHATWG Streams, written from the Streams Standard (no reference-implementation code).
// This file is the first fragment of one script: lib.rs concatenates 00..70 so every part shares
// the private scope opened here and closed in 70_exports.js. Evaluated once per isolate.
//
// Design: the public classes are thin shells. Their state lives in plain "records" kept in a
// WeakMap (`records`), so the abstract operations of the standard map one-to-one onto functions
// that take records. Nothing reachable from user code exposes a record.
(function (global) {
  "use strict";

  // Capture the raw ops, then remove them so user code never sees them (ADR-010).
  const flateOp = global.op_streams_flate;
  const flateFreeOp = global.op_streams_flate_free;
  delete global.op_streams_flate;
  delete global.op_streams_flate_free;

  const reflectApply = Reflect.apply;
  const promiseThen = Promise.prototype.then;
  const bufferTransfer = ArrayBuffer.prototype.transfer;
  const bufferDetached = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "detached").get;

  const records = new WeakMap();
  let allowConstruct = false;

  function brand(object, kind, name) {
    const record = records.get(object);
    if (record === undefined || record.kind !== kind) {
      throw new TypeError(`Illegal invocation: receiver is not a ${name}`);
    }
    return record;
  }

  // Public classes without a user-facing constructor (controllers, readers' requests).
  function construct(Class, record) {
    allowConstruct = true;
    let wrapper;
    try {
      wrapper = new Class();
    } finally {
      allowConstruct = false;
    }
    record.wrapper = wrapper;
    records.set(wrapper, record);
    return wrapper;
  }

  function assertConstructible() {
    if (!allowConstruct) throw new TypeError("Illegal constructor");
  }

  // WebIDL shape: operations and attributes are enumerable, the class has a toStringTag.
  function finishClass(Class, tag) {
    for (const key of Reflect.ownKeys(Class.prototype)) {
      if (key === "constructor") continue;
      const descriptor = Object.getOwnPropertyDescriptor(Class.prototype, key);
      descriptor.enumerable = true;
      Object.defineProperty(Class.prototype, key, descriptor);
    }
    Object.defineProperty(Class.prototype, Symbol.toStringTag, { value: tag, configurable: true });
  }

  function define(name, value) {
    Object.defineProperty(global, name, {
      value,
      writable: true,
      configurable: true,
      enumerable: false,
    });
  }

  // ---- promises ----

  const resolved = (value) => Promise.resolve(value);
  const rejected = (reason) => Promise.reject(reason);

  function markHandled(promise) {
    reflectApply(promiseThen, promise, [undefined, () => {}]);
    return promise;
  }

  function upon(promise, onFulfilled, onRejected) {
    return reflectApply(promiseThen, promise, [onFulfilled, onRejected]);
  }

  // A promise plus its settle functions; `pending` tells whether it has been settled.
  function defer() {
    const deferred = { pending: true };
    deferred.promise = new Promise((resolve, reject) => {
      deferred.resolve = (value) => {
        deferred.pending = false;
        resolve(value);
      };
      deferred.reject = (reason) => {
        deferred.pending = false;
        reject(reason);
      };
    });
    return deferred;
  }

  function deferResolved(value) {
    const deferred = defer();
    deferred.resolve(value);
    return deferred;
  }

  function deferRejected(reason) {
    const deferred = defer();
    deferred.reject(reason);
    markHandled(deferred.promise);
    return deferred;
  }

  // Calls `fn` and always returns a promise; a synchronous throw becomes a rejection.
  function promiseCall(fn, thisArg, args) {
    try {
      return Promise.resolve(reflectApply(fn, thisArg, args));
    } catch (error) {
      return Promise.reject(error);
    }
  }

  // Wraps the body of a promise-returning method so brand-check failures reject instead of throw.
  function guard(body) {
    try {
      return body();
    } catch (error) {
      return Promise.reject(error);
    }
  }

  // ---- argument conversion ----

  function toDict(value, name) {
    if (value === undefined || value === null) return {};
    if (typeof value !== "object" && typeof value !== "function") {
      throw new TypeError(`${name} must be an object`);
    }
    return value;
  }

  function optionalCallback(dict, key, name) {
    const fn = dict[key];
    if (fn === undefined) return undefined;
    if (typeof fn !== "function") throw new TypeError(`${name}.${key} must be a function`);
    return fn;
  }

  function extractHighWaterMark(strategy, defaultValue) {
    const raw = strategy.highWaterMark;
    if (raw === undefined) return defaultValue;
    const value = Number(raw);
    if (Number.isNaN(value) || value < 0) throw new RangeError("Invalid highWaterMark");
    return value;
  }

  function extractSizeAlgorithm(strategy) {
    const size = strategy.size;
    if (size === undefined) return () => 1;
    if (typeof size !== "function") throw new TypeError("strategy.size must be a function");
    return (chunk) => reflectApply(size, undefined, [chunk]);
  }

  // ---- queues with sizes ----

  function enqueueValueWithSize(container, value, size) {
    size = Number(size);
    if (Number.isNaN(size) || size < 0 || size === Infinity) {
      throw new RangeError("The chunk size must be a finite, non-negative number");
    }
    container.queue.push({ value, size });
    container.queueTotalSize += size;
  }

  function dequeueValueWithSize(container) {
    const entry = container.queue.shift();
    container.queueTotalSize = Math.max(0, container.queueTotalSize - entry.size);
    return entry.value;
  }

  function peekQueueValue(container) {
    return container.queue[0].value;
  }

  function resetQueue(container) {
    container.queue = [];
    container.queueTotalSize = 0;
  }

  // ---- buffers ----

  function isDetached(buffer) {
    return reflectApply(bufferDetached, buffer, []);
  }

  function transferBuffer(buffer) {
    if (isDetached(buffer)) throw new TypeError("The buffer is detached");
    return reflectApply(bufferTransfer, buffer, []);
  }

  function cloneAsUint8Array(view) {
    return new Uint8Array(view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength));
  }

  function copyBytes(source, sourceOffset, target, targetOffset, length) {
    new Uint8Array(target, targetOffset, length).set(new Uint8Array(source, sourceOffset, length));
  }

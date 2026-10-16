'use strict';
// node:async_hooks: AsyncLocalStorage, a minimal AsyncResource and a createHook stub. The store
// lives in the frame of internal/async_context. Written for this runtime.
//
// Known limits: createHook never calls its hooks (it warns and returns an inert object);
// executionAsyncId/triggerAsyncId are always 0; on an engine without a continuation context the
// store is lost after an `await` (one warning per process says so).
const errors = require('internal/errors');
const context = require('internal/async_context');
const process = require('process');

let warnedAboutAwait = false;
function noteEngineLimit() {
  if (context.supported || warnedAboutAwait) return;
  warnedAboutAwait = true;
  process.emitWarning(
    'AsyncLocalStorage does not follow `await` on this engine: the store is lost after the first await',
    'AsyncLocalStorageWarning',
  );
}

// A copy of the current frame with `store` set for `storage` (or removed when `remove`).
function frameWith(storage, store, remove) {
  const next = new Map(context.current());
  if (remove) next.delete(storage);
  else next.set(storage, store);
  return next;
}

let nextResourceId = 1;

class AsyncResource {
  #frame = context.current();
  #id = nextResourceId++;

  constructor(type) {
    errors.validateString(type, 'type');
    this.type = type;
  }

  runInAsyncScope(fn, thisArg, ...args) {
    return context.run(this.#frame, fn, thisArg, args);
  }

  emitDestroy() { return this; }
  asyncId() { return this.#id; }
  triggerAsyncId() { return 0; }

  bind(fn, thisArg) {
    errors.validateFunction(fn, 'fn');
    const resource = this;
    const bound = function (...args) {
      return resource.runInAsyncScope(fn, thisArg === undefined ? this : thisArg, ...args);
    };
    Object.defineProperties(bound, {
      length: { value: fn.length, configurable: true },
      asyncResource: { value: this, configurable: true },
    });
    return bound;
  }

  static bind(fn, type, thisArg) {
    errors.validateFunction(fn, 'fn');
    return new AsyncResource(type || fn.name || 'bound-anonymous-fn').bind(fn, thisArg);
  }
}

class AsyncLocalStorage {
  #disabled = false;

  static bind(fn) { return AsyncResource.bind(fn); }

  static snapshot() {
    const frame = context.current();
    return (fn, ...args) => context.run(frame, fn, undefined, args);
  }

  run(store, callback, ...args) {
    errors.validateFunction(callback, 'callback');
    noteEngineLimit();
    this.#disabled = false;
    return context.run(frameWith(this, store, false), callback, undefined, args);
  }

  exit(callback, ...args) {
    errors.validateFunction(callback, 'callback');
    return context.run(frameWith(this, undefined, true), callback, undefined, args);
  }

  enterWith(store) {
    noteEngineLimit();
    this.#disabled = false;
    context.enter(frameWith(this, store, false));
  }

  getStore() {
    if (this.#disabled) return undefined;
    const frame = context.current();
    return frame === undefined ? undefined : frame.get(this);
  }

  disable() { this.#disabled = true; }
}

function createHook() {
  process.emitWarning(
    'async_hooks.createHook is not supported: the hooks are never called',
    'AsyncHooksWarning',
  );
  const hook = { enable: () => hook, disable: () => hook };
  return hook;
}

module.exports = {
  AsyncLocalStorage,
  AsyncResource,
  createHook,
  executionAsyncId: () => 0,
  executionAsyncResource: () => ({}),
  triggerAsyncId: () => 0,
};

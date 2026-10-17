'use strict';
// AsyncDisposableStack (explicit resource management proposal). Mirrors disposable.js with awaited disposal.
(() => {
  const { defineProperty } = Object;
  const stacks = new WeakMap(); // stack -> { disposed, resources: [{ value, method }] }; method null = await only
  const define = (target, methods) => {
    for (const name of Object.keys(methods)) {
      defineProperty(target, name, { value: methods[name], writable: true, configurable: true });
    }
  };
  const isObject = (v) => (typeof v === 'object' && v !== null) || typeof v === 'function';
  const pending = (stack) => {
    const state = stacks.get(stack);
    if (state === undefined) throw new TypeError('receiver is not an AsyncDisposableStack');
    if (state.disposed) throw new ReferenceError('AsyncDisposableStack is already disposed');
    return state;
  };
  const callable = (method, what) => {
    if (typeof method !== 'function') throw new TypeError(`${what} is not a function`);
    return method;
  };
  // GetDisposeMethod with the async hint: @@asyncDispose, else a wrapper over @@dispose that discards its result.
  const disposeMethod = (value) => {
    const asyncMethod = value[Symbol.asyncDispose];
    if (asyncMethod != null) return callable(asyncMethod, 'Symbol.asyncDispose');
    const syncMethod = value[Symbol.dispose];
    if (syncMethod == null) throw new TypeError('value is not disposable');
    callable(syncMethod, 'Symbol.dispose');
    return function () { Reflect.apply(syncMethod, this, []); };
  };

  class AsyncDisposableStack {
    constructor() { stacks.set(this, { disposed: false, resources: [] }); }
  }

  const methods = {
    use(value) {
      const state = pending(this);
      if (value == null) {
        state.resources.push({ value: undefined, method: null });
        return value;
      }
      if (!isObject(value)) throw new TypeError('value is not an object');
      state.resources.push({ value, method: disposeMethod(value) });
      return value;
    },
    adopt(value, onDispose) {
      const state = pending(this);
      callable(onDispose, 'onDispose');
      state.resources.push({ value: undefined, method: () => onDispose(value) });
      return value;
    },
    defer(onDispose) {
      const state = pending(this);
      callable(onDispose, 'onDispose');
      state.resources.push({ value: undefined, method: onDispose });
    },
    move() {
      const state = pending(this);
      const moved = new AsyncDisposableStack();
      stacks.get(moved).resources = state.resources;
      state.resources = [];
      state.disposed = true;
      return moved;
    },
    async disposeAsync() {
      const state = stacks.get(this);
      if (state === undefined) throw new TypeError('receiver is not an AsyncDisposableStack');
      if (state.disposed) return undefined;
      state.disposed = true;
      const { resources } = state;
      state.resources = [];
      let failed = false;
      let error;
      if (resources.length === 0) await undefined;
      for (let i = resources.length - 1; i >= 0; i--) {
        const { value, method } = resources[i];
        try {
          await (method === null ? undefined : Reflect.apply(method, value, []));
        } catch (thrown) {
          error = failed ? new SuppressedError(thrown, error) : thrown;
          failed = true;
        }
      }
      if (failed) throw error;
      return undefined;
    },
  };
  define(AsyncDisposableStack.prototype, methods);
  defineProperty(AsyncDisposableStack.prototype, Symbol.asyncDispose, {
    value: methods.disposeAsync, writable: true, configurable: true,
  });
  const getDisposed = {
    get disposed() {
      const state = stacks.get(this);
      if (state === undefined) throw new TypeError('receiver is not an AsyncDisposableStack');
      return state.disposed;
    },
  };
  defineProperty(AsyncDisposableStack.prototype, 'disposed', {
    get: Object.getOwnPropertyDescriptor(getDisposed, 'disposed').get, configurable: true,
  });
  defineProperty(AsyncDisposableStack.prototype, Symbol.toStringTag, {
    value: 'AsyncDisposableStack', configurable: true,
  });
  defineProperty(globalThis, 'AsyncDisposableStack', { value: AsyncDisposableStack, writable: true, configurable: true });
})();

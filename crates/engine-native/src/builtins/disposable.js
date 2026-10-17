'use strict';
// DisposableStack (explicit resource management proposal). State lives in a WeakMap keyed by the stack.
(() => {
  const { defineProperty } = Object;
  const stacks = new WeakMap(); // stack -> { disposed, resources: [{ value, method }] }
  const define = (target, methods) => {
    for (const name of Object.keys(methods)) {
      defineProperty(target, name, { value: methods[name], writable: true, configurable: true });
    }
  };
  const isObject = (v) => (typeof v === 'object' && v !== null) || typeof v === 'function';
  const pending = (stack) => {
    const state = stacks.get(stack);
    if (state === undefined) throw new TypeError('receiver is not a DisposableStack');
    if (state.disposed) throw new ReferenceError('DisposableStack is already disposed');
    return state;
  };

  class DisposableStack {
    constructor() { stacks.set(this, { disposed: false, resources: [] }); }
  }

  const methods = {
    use(value) {
      const state = pending(this);
      if (value == null) return value;
      if (!isObject(value)) throw new TypeError('value is not an object');
      const method = value[Symbol.dispose];
      if (typeof method !== 'function') throw new TypeError('value is not disposable');
      state.resources.push({ value, method });
      return value;
    },
    adopt(value, onDispose) {
      const state = pending(this);
      if (typeof onDispose !== 'function') throw new TypeError('onDispose is not a function');
      state.resources.push({ value: undefined, method: () => onDispose(value) });
      return value;
    },
    defer(onDispose) {
      const state = pending(this);
      if (typeof onDispose !== 'function') throw new TypeError('onDispose is not a function');
      state.resources.push({ value: undefined, method: onDispose });
    },
    move() {
      const state = pending(this);
      const moved = new DisposableStack();
      stacks.get(moved).resources = state.resources;
      state.resources = [];
      state.disposed = true;
      return moved;
    },
    dispose() {
      const state = stacks.get(this);
      if (state === undefined) throw new TypeError('receiver is not a DisposableStack');
      if (state.disposed) return undefined;
      state.disposed = true;
      const { resources } = state;
      state.resources = [];
      let failed = false;
      let error;
      for (let i = resources.length - 1; i >= 0; i--) {
        const { value, method } = resources[i];
        try {
          Reflect.apply(method, value, []);
        } catch (thrown) {
          error = failed ? new SuppressedError(thrown, error) : thrown;
          failed = true;
        }
      }
      if (failed) throw error;
      return undefined;
    },
  };
  define(DisposableStack.prototype, methods);
  defineProperty(DisposableStack.prototype, Symbol.dispose, {
    value: methods.dispose, writable: true, configurable: true,
  });
  const getDisposed = {
    get disposed() {
      const state = stacks.get(this);
      if (state === undefined) throw new TypeError('receiver is not a DisposableStack');
      return state.disposed;
    },
  };
  defineProperty(DisposableStack.prototype, 'disposed', {
    get: Object.getOwnPropertyDescriptor(getDisposed, 'disposed').get, configurable: true,
  });
  defineProperty(DisposableStack.prototype, Symbol.toStringTag, { value: 'DisposableStack', configurable: true });
  defineProperty(globalThis, 'DisposableStack', { value: DisposableStack, writable: true, configurable: true });
})();

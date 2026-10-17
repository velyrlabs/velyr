'use strict';
// Array.fromAsync (ECMA-262 2026 23.1.2.2): the spec steps written out with one `await` per Await.
// shortcut: sync iterables are adapted by hand, not by the engine's AsyncFromSyncIterator.
(() => {
  const { apply } = Reflect;
  const { defineProperty } = Object;
  const PromiseCtor = Promise;
  const promiseResolve = PromiseCtor.resolve.bind(PromiseCtor);
  const promiseThen = Function.prototype.call.bind(PromiseCtor.prototype.then);
  const isObject = (v) => (typeof v === 'object' && v !== null) || typeof v === 'function';
  const isCallable = (v) => typeof v === 'function';
  const getMethod = (target, key) => {
    const method = target[key];
    if (method == null) return undefined;
    if (!isCallable(method)) throw new TypeError('method is not callable');
    return method;
  };
  const isConstructor = (v) => {
    if (!isCallable(v)) return false;
    try { Reflect.construct(Object, [], v); return true; } catch { return false; }
  };

  const closeSync = (iterator) => {
    try { const ret = iterator.return; if (ret != null) apply(ret, iterator, []); } catch (closeError) { void closeError; /* IteratorClose: the original throw completion wins (7.4.11 step 5) */ }
  };
  // CreateAsyncFromSyncIterator: next() yields a promise of the awaited step result.
  const fromSync = (iterator, next) => ({
    next() {
      const result = apply(next, iterator, []);
      if (!isObject(result)) throw new TypeError('iterator result is not an object');
      const done = !!result.done;
      const wrapper = promiseResolve(result.value);
      return promiseThen(wrapper, (value) => ({ value, done }), (error) => {
        if (!done) closeSync(iterator);
        throw error;
      });
    },
    return: undefined,
    sync: iterator,
  });

  const closeAsync = async (record, original) => {
    try {
      const ret = record.sync ? undefined : record.iterator.return;
      if (record.sync) closeSync(record.sync);
      else if (ret != null) await apply(ret, record.iterator, []);
    } catch (closeError) { void closeError; /* IteratorClose: the original throw completion wins (7.4.11 step 5) */ }
    throw original;
  };

  const run = async (C, items, mapfn, thisArg) => {
    const mapping = mapfn !== undefined;
    if (mapping && !isCallable(mapfn)) throw new TypeError('mapfn is not callable');
    let record;
    const asyncMethod = getMethod(items, Symbol.asyncIterator);
    if (asyncMethod !== undefined) {
      const iterator = apply(asyncMethod, items, []);
      if (!isObject(iterator)) throw new TypeError('iterator is not an object');
      record = { iterator, next: iterator.next };
    } else {
      const syncMethod = getMethod(items, Symbol.iterator);
      if (syncMethod !== undefined) {
        const iterator = apply(syncMethod, items, []);
        if (!isObject(iterator)) throw new TypeError('iterator is not an object');
        const adapter = fromSync(iterator, iterator.next);
        record = { iterator: adapter, next: adapter.next, sync: iterator };
      }
    }
    if (record !== undefined) {
      const A = isConstructor(C) ? new C() : [];
      for (let k = 0; ; k++) {
        const step = await apply(record.next, record.iterator, []);
        if (!isObject(step)) throw new TypeError('iterator result is not an object');
        if (step.done) { A.length = k; return A; }
        let value = step.value;
        try {
          if (mapping) value = await apply(mapfn, thisArg, [value, k]);
          defineProperty(A, k, { value, writable: true, enumerable: true, configurable: true });
        } catch (error) {
          await closeAsync(record, error);
        }
      }
    }
    const arrayLike = Object(items);
    const len = Math.min(Math.max(Math.trunc(Number(arrayLike.length)) || 0, 0), Number.MAX_SAFE_INTEGER);
    const A = isConstructor(C) ? new C(len) : new Array(len);
    for (let k = 0; k < len; k++) {
      let value = await arrayLike[k];
      if (mapping) value = await apply(mapfn, thisArg, [value, k]);
      defineProperty(A, k, { value, writable: true, enumerable: true, configurable: true });
    }
    A.length = len;
    return A;
  };

  const methods = {
    fromAsync(asyncItems) {
      return run(this, asyncItems, arguments[1], arguments[2]);
    },
  };
  defineProperty(Array, 'fromAsync', { value: methods.fromAsync, writable: true, configurable: true });
})();

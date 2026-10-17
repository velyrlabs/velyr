//! ES2025 iterator helpers (VELYR-109): the `Iterator` constructor, `Iterator.from` and
//! `Iterator.prototype.{map, filter, take, drop, flatMap, reduce, toArray, forEach, some,
//! every, find, chunks, windows, includes, join}` plus `Iterator.{concat, zip, zipKeyed}`. Written in JS and run once per realm: the lazy helpers are generators
//! behind %IteratorHelperPrototype% objects, so they get suspension and closing for free.
//! shortcut: `toString` of these functions shows their JS source, not `[native code]`.

use velyr_engine_vm::interpreter::Interpreter;

const PRELUDE: &str = r#"'use strict';
(() => {
  const { apply } = Reflect;
  const { create, defineProperty, getPrototypeOf } = Object;
  const isPrototypeOf = Function.prototype.call.bind(Object.prototype.isPrototypeOf);
  const IteratorPrototype = getPrototypeOf(getPrototypeOf([][Symbol.iterator]()));
  const DONE = {};
  const define = (target, methods) => {
    for (const name of Object.keys(methods)) {
      defineProperty(target, name, { value: methods[name], writable: true, configurable: true });
    }
  };
  const isObject = (v) => (typeof v === 'object' && v !== null) || typeof v === 'function';
  const fail = (message) => { throw new TypeError(message); };
  const closeQuietly = (iterator) => {
    try { const close = iterator.return; if (close != null) apply(close, iterator, []); } catch {}
  };
  const close = (iterator) => {
    const method = iterator.return;
    if (method == null) return;
    if (!isObject(apply(method, iterator, []))) fail('Iterator result is not an object');
  };
  // GetIteratorDirect after the argument checks (which close `O` when they fail).
  const direct = (O, check) => {
    if (!isObject(O)) fail('Iterator helper called on a non-object');
    try { check(); } catch (error) { closeQuietly(O); throw error; }
    return { iterator: O, next: O.next };
  };
  // Messages never interpolate the argument: that would run its `toString`/`valueOf`.
  const callable = (f) => () => { if (typeof f !== 'function') fail('argument is not a function'); };
  const limit = (value) => {
    const n = Number(value);
    if (n !== n) throw new RangeError('limit must be a number');
    const integer = Math.trunc(n);
    if (integer < 0 || (integer > Number.MAX_SAFE_INTEGER && integer !== Infinity)) {
      throw new RangeError('limit is out of range');
    }
    return integer;
  };
  // A Number with an integral value in [min, max] (no coercion); `Infinity` only when allowed.
  const integerArg = (value, min, max, allowInfinity) => () => {
    if (typeof value !== 'number' || value !== value) fail('argument must be a number');
    if (value === Infinity && allowInfinity) return;
    if (Math.trunc(value) !== value || (!allowInfinity && Math.abs(value) === Infinity)) {
      fail('argument must be an integer');
    }
    if (value < min || value > max) throw new RangeError('argument is out of range');
  };
  const MAX_CHUNK = 2 ** 32 - 1;
  const stepValue = (record) => {
    const result = apply(record.next, record.iterator, []);
    if (!isObject(result)) fail('Iterator result ' + result + ' is not an object');
    return result.done ? DONE : result.value;
  };
  const callClosing = (record, f, args) => {
    try { return apply(f, undefined, args); } catch (error) { closeQuietly(record.iterator); throw error; }
  };
  // Yields `value`; a `return()` while suspended closes the iterators, innermost first.
  function* emit(value, ...records) {
    let resumed = false;
    try { yield value; resumed = true; } finally {
      if (!resumed) {
        try { if (records.length > 1) close(records[0].iterator); }
        catch (error) { closeQuietly(records[1].iterator); throw error; }
        close(records[records.length - 1].iterator);
      }
    }
  }
  const getIteratorFlattenable = (value, allowStrings) => {
    if (!isObject(value) && !(allowStrings && typeof value === 'string')) {
      fail(value + ' is not an object');
    }
    const method = value[Symbol.iterator];
    const iterator = method == null ? value : apply(method, value, []);
    if (!isObject(iterator)) fail(iterator + ' is not an object');
    return { iterator, next: iterator.next };
  };

  const helpers = new WeakMap();
  const helperState = (O) => helpers.get(O) || fail('not an Iterator Helper');
  const IteratorHelperPrototype = create(IteratorPrototype);
  define(IteratorHelperPrototype, {
    next() { const state = helperState(this); state.started = true; return state.generator.next(); },
    return() {
      const state = helperState(this);
      if (!state.started) {
        state.started = true;
        state.generator.return();
        close(state.record.iterator);
        return { value: undefined, done: true };
      }
      return state.generator.return();
    },
  });
  defineProperty(IteratorHelperPrototype, Symbol.toStringTag, { value: 'Iterator Helper', configurable: true });
  const helper = (record, body) => {
    const object = create(IteratorHelperPrototype);
    helpers.set(object, { record, started: false, generator: body(record) });
    return object;
  };

  function Iterator() {
    if (new.target === undefined || new.target === Iterator) {
      fail('Abstract class Iterator not directly constructable');
    }
  }
  defineProperty(Iterator, 'prototype', { value: IteratorPrototype, writable: false });
  // SetterThatIgnoresPrototypeProperties(this, %Iterator.prototype%, 'constructor', v).
  const { get, set } = Object.getOwnPropertyDescriptor({
    get constructor() { return Iterator; },
    set constructor(value) {
      if (!isObject(this)) fail('Iterator.prototype.constructor setter called on non-object');
      if (this === IteratorPrototype) fail("Cannot assign to read only property 'constructor'");
      if (Object.getOwnPropertyDescriptor(this, 'constructor') === undefined) {
        defineProperty(this, 'constructor', { value, writable: true, enumerable: true, configurable: true });
      } else {
        this.constructor = value;
      }
    },
  }, 'constructor');
  defineProperty(IteratorPrototype, 'constructor', { get, set, configurable: true });

  const wrapped = new WeakMap();
  const WrapForValidIteratorPrototype = create(IteratorPrototype);
  define(WrapForValidIteratorPrototype, {
    next() {
      const record = wrapped.get(this) || fail('not a wrapped iterator');
      return apply(record.next, record.iterator, []);
    },
    return() {
      const record = wrapped.get(this) || fail('not a wrapped iterator');
      const method = record.iterator.return;
      return method === undefined ? { value: undefined, done: true } : apply(method, record.iterator, []);
    },
  });
  define(Iterator, {
    from(O) {
      const record = getIteratorFlattenable(O, true);
      if (isPrototypeOf(IteratorPrototype, record.iterator)) return record.iterator;
      const wrapper = create(WrapForValidIteratorPrototype);
      wrapped.set(wrapper, record);
      return wrapper;
    },
  });

  // IteratorCloseAll: reverse order; with a normal completion the first close error wins.
  const closeAll = (records, abrupt) => {
    let failure;
    let failed = false;
    for (let i = records.length - 1; i >= 0; i--) {
      try { close(records[i].iterator); } catch (error) {
        if (!abrupt && !failed) { failed = true; failure = error; }
      }
    }
    if (failed) throw failure;
  };
  // A helper that is closed before its first `next()` closes `records` instead of a source.
  const closer = (records) => ({ iterator: { return() { closeAll(records, false); return {}; } } });
  const getIterator = (value) => {
    const method = value[Symbol.iterator];
    if (typeof method !== 'function') fail('value is not iterable');
    const iterator = apply(method, value, []);
    if (!isObject(iterator)) fail('iterator is not an object');
    return { iterator, next: iterator.next };
  };
  // Iterator.zip / zipKeyed: `names` is undefined for zip, the property keys for zipKeyed.
  function* zipBody(records, mode, padding, names) {
    const count = records.length;
    const done = records.map(() => false);
    const open = () => records.filter((_, j) => !done[j]);
    const others = (i) => records.filter((_, j) => j !== i && !done[j]);
    const stepAt = (i) => {
      done[i] = true;
      let value;
      try { value = stepValue(records[i]); } catch (error) { closeAll(others(i), true); throw error; }
      if (value !== DONE) done[i] = false;
      return value;
    };
    if (count === 0) return;
    for (;;) {
      const values = [];
      for (let i = 0; i < count; i++) {
        if (done[i]) { values.push(padding[i]); continue; }
        const value = stepAt(i);
        if (value !== DONE) { values.push(value); continue; }
        if (mode === 'shortest') return closeAll(others(i), false);
        if (mode === 'strict') {
          if (i !== 0) { closeAll(open(), true); fail('iterables have different lengths'); }
          for (let j = 1; j < count; j++) {
            if (stepAt(j) !== DONE) { closeAll(open(), true); fail('iterables have different lengths'); }
          }
          return;
        }
        if (done.every(Boolean)) return;
        values.push(padding[i]);
      }
      let result = values;
      if (names !== undefined) {
        result = create(null);
        names.forEach((name, i) => { result[name] = values[i]; });
      }
      let suspended = true;
      try { yield result; suspended = false; } finally { if (suspended) closeAll(open(), false); }
    }
  }
  const zipHelper = (iterables, options, keyed) => {
    if (!isObject(iterables)) fail('iterables must be an object');
    if (options !== undefined && !isObject(options)) fail('options must be an object');
    let mode = options === undefined ? undefined : options.mode;
    if (mode === undefined) mode = 'shortest';
    if (mode !== 'shortest' && mode !== 'longest' && mode !== 'strict') fail('invalid zip mode');
    let paddingOption;
    if (mode === 'longest' && options !== undefined) {
      paddingOption = options.padding;
      if (paddingOption !== undefined && !isObject(paddingOption)) fail('padding must be an object');
    }
    const records = [];
    const names = keyed ? [] : undefined;
    const padding = [];
    try {
      if (keyed) {
        for (const key of Reflect.ownKeys(iterables)) {
          const descriptor = Reflect.getOwnPropertyDescriptor(iterables, key);
          if (descriptor === undefined || !descriptor.enumerable) continue;
          const value = iterables[key];
          if (value === undefined) continue;
          records.push(getIteratorFlattenable(value, false));
          names.push(key);
        }
      } else {
        const outer = getIterator(iterables);
        for (let item; (item = stepValue(outer)) !== DONE; ) {
          try { records.push(getIteratorFlattenable(item, false)); } catch (error) {
            closeAll([outer], true);
            throw error;
          }
        }
      }
      for (let i = 0; i < records.length; i++) padding.push(undefined);
      if (paddingOption !== undefined && keyed) {
        names.forEach((name, i) => { padding[i] = paddingOption[name]; });
      } else if (paddingOption !== undefined) {
        const source = getIterator(paddingOption);
        let using = true;
        for (let i = 0; i < records.length && using; i++) {
          const value = stepValue(source);
          if (value === DONE) using = false; else padding[i] = value;
        }
        if (using) close(source.iterator);
      }
    } catch (error) {
      closeAll(records, true);
      throw error;
    }
    return helper(closer(records), () => zipBody(records, mode, padding, names));
  };
  define(Iterator, {
    concat(...items) {
      const sources = [];
      for (let i = 0; i < items.length; i++) {
        const item = items[i];
        if (!isObject(item)) fail('Iterator.concat argument is not an object');
        const method = item[Symbol.iterator];
        if (typeof method !== 'function') fail('Iterator.concat argument is not iterable');
        sources.push({ item, method });
      }
      return helper(closer([]), function* () {
        for (const { item, method } of sources) {
          const iterator = apply(method, item, []);
          if (!isObject(iterator)) fail('iterator is not an object');
          const record = { iterator, next: iterator.next };
          for (let value; (value = stepValue(record)) !== DONE; ) {
            let suspended = true;
            try { yield value; suspended = false; } finally { if (suspended) close(record.iterator); }
          }
        }
      });
    },
    zip(iterables) { return zipHelper(iterables, arguments[1], false); },
    zipKeyed(iterables) { return zipHelper(iterables, arguments[1], true); },
  });

  define(IteratorPrototype, {
    map(mapper) {
      return helper(direct(this, callable(mapper)), function* (record) {
        for (let counter = 0; ; counter++) {
          const value = stepValue(record);
          if (value === DONE) return;
          yield* emit(callClosing(record, mapper, [value, counter]), record);
        }
      });
    },
    filter(predicate) {
      return helper(direct(this, callable(predicate)), function* (record) {
        for (let counter = 0; ; counter++) {
          const value = stepValue(record);
          if (value === DONE) return;
          if (callClosing(record, predicate, [value, counter])) yield* emit(value, record);
        }
      });
    },
    take(count) {
      let remaining;
      return helper(direct(this, () => { remaining = limit(count); }), function* (record) {
        for (;;) {
          if (remaining === 0) return close(record.iterator);
          if (remaining !== Infinity) remaining--;
          const value = stepValue(record);
          if (value === DONE) return;
          yield* emit(value, record);
        }
      });
    },
    drop(count) {
      let remaining;
      return helper(direct(this, () => { remaining = limit(count); }), function* (record) {
        for (; remaining > 0; remaining !== Infinity && remaining--) {
          if (stepValue(record) === DONE) return;
        }
        for (;;) {
          const value = stepValue(record);
          if (value === DONE) return;
          yield* emit(value, record);
        }
      });
    },
    flatMap(mapper) {
      return helper(direct(this, callable(mapper)), function* (record) {
        for (let counter = 0; ; counter++) {
          const value = stepValue(record);
          if (value === DONE) return;
          const mapped = callClosing(record, mapper, [value, counter]);
          const inner = callClosing(record, getIteratorFlattenable, [mapped, false]);
          for (;;) {
            const innerValue = callClosing(record, stepValue, [inner]);
            if (innerValue === DONE) break;
            yield* emit(innerValue, inner, record);
          }
        }
      });
    },
    chunks(chunkSize) {
      return helper(direct(this, integerArg(chunkSize, 1, MAX_CHUNK)), function* (record) {
        let buffer = [];
        for (;;) {
          const value = stepValue(record);
          if (value === DONE) break;
          buffer.push(value);
          if (buffer.length === chunkSize) {
            const full = buffer;
            buffer = [];
            yield* emit(full, record);
          }
        }
        if (buffer.length > 0) yield buffer;
      });
    },
    windows(windowSize) {
      const undersized = arguments[1];
      const validate = () => {
        integerArg(windowSize, 1, MAX_CHUNK)();
        if (undersized !== undefined && undersized !== 'only-full' && undersized !== 'allow-partial') {
          fail("undersized must be 'only-full' or 'allow-partial'");
        }
      };
      return helper(direct(this, validate), function* (record) {
        const buffer = [];
        for (;;) {
          const value = stepValue(record);
          if (value === DONE) break;
          buffer.push(value);
          if (buffer.length > windowSize) buffer.shift();
          if (buffer.length === windowSize) yield* emit(buffer.slice(), record);
        }
        if (undersized === 'allow-partial' && buffer.length > 0 && buffer.length < windowSize) {
          yield buffer;
        }
      });
    },
    includes(searchElement) {
      let skipped = arguments[1] === undefined ? 0 : arguments[1];
      const record = direct(this, integerArg(skipped, 0, Number.MAX_SAFE_INTEGER, true));
      for (let value; (value = stepValue(record)) !== DONE; ) {
        if (skipped > 0) { skipped--; continue; }
        if (value === searchElement || (value !== value && searchElement !== searchElement)) {
          close(record.iterator);
          return true;
        }
      }
      return false;
    },
    join(separator) {
      let text;
      const record = direct(this, () => { text = separator === undefined ? ',' : `${separator}`; });
      let result = '';
      for (let first = true, value; (value = stepValue(record)) !== DONE; first = false) {
        let part = '';
        try { if (value != null) part = `${value}`; } catch (error) { closeQuietly(record.iterator); throw error; }
        result += first ? part : text + part;
      }
      return result;
    },
    reduce(reducer) {
      const record = direct(this, callable(reducer));
      let counter = 0;
      let accumulator = arguments[1];
      if (arguments.length < 2) {
        accumulator = stepValue(record);
        if (accumulator === DONE) fail('Reduce of empty iterator with no initial value');
        counter = 1;
      }
      for (;; counter++) {
        const value = stepValue(record);
        if (value === DONE) return accumulator;
        accumulator = callClosing(record, reducer, [accumulator, value, counter]);
      }
    },
    toArray() {
      const record = direct(this, () => {});
      const items = [];
      for (let value; (value = stepValue(record)) !== DONE; ) items.push(value);
      return items;
    },
    forEach(fn) {
      const record = direct(this, callable(fn));
      for (let counter = 0, value; (value = stepValue(record)) !== DONE; counter++) {
        callClosing(record, fn, [value, counter]);
      }
    },
    some(predicate) {
      const record = direct(this, callable(predicate));
      for (let counter = 0, value; (value = stepValue(record)) !== DONE; counter++) {
        if (callClosing(record, predicate, [value, counter])) { close(record.iterator); return true; }
      }
      return false;
    },
    every(predicate) {
      const record = direct(this, callable(predicate));
      for (let counter = 0, value; (value = stepValue(record)) !== DONE; counter++) {
        if (!callClosing(record, predicate, [value, counter])) { close(record.iterator); return false; }
      }
      return true;
    },
    find(predicate) {
      const record = direct(this, callable(predicate));
      for (let counter = 0, value; (value = stepValue(record)) !== DONE; counter++) {
        if (callClosing(record, predicate, [value, counter])) { close(record.iterator); return value; }
      }
    },
  });
  // Explicit resource management hooks; the computed keys give the `[Symbol.*]` function names.
  const AsyncIteratorPrototype = getPrototypeOf(getPrototypeOf(async function* () {}.prototype));
  const getReturn = (iterator) => {
    const method = iterator.return;
    if (method != null && typeof method !== 'function') fail('return is not a function');
    return method;
  };
  const disposers = {
    [Symbol.dispose]() {
      const method = getReturn(this);
      if (method != null) apply(method, this, []);
    },
    async [Symbol.asyncDispose]() {
      const method = getReturn(this);
      if (method != null) await apply(method, this, []);
    },
  };
  for (const [target, key] of [[IteratorPrototype, Symbol.dispose], [AsyncIteratorPrototype, Symbol.asyncDispose]]) {
    defineProperty(target, key, { value: disposers[key], writable: true, configurable: true });
  }
  defineProperty(globalThis, 'Iterator', { value: Iterator, writable: true, configurable: true });
})();
"#;

/// Defines `Iterator` and the iterator helpers on the realm of `vm`.
pub fn install(vm: &mut Interpreter) -> Result<(), String> {
    crate::prelude::run_prelude(vm, "velyr:iterator_helpers", PRELUDE)
}

'use strict';
// node:events. Listener storage mirrors Node (`_events`, `_eventsCount`, `_maxListeners`) so code
// that pokes at those fields keeps working.
const errors = require('internal/errors');

const kCapture = Symbol('kCapture');
const kRejection = Symbol.for('nodejs.rejection');
const errorMonitor = Symbol('events.errorMonitor');
let defaultMaxListeners = 10;

function EventEmitter(opts) {
  EventEmitter.init.call(this, opts);
}

EventEmitter.prototype._events = undefined;
EventEmitter.prototype._eventsCount = 0;
EventEmitter.prototype._maxListeners = undefined;

EventEmitter.init = function init(opts) {
  if (this._events === undefined || this._events === Object.getPrototypeOf(this)._events) {
    this._events = { __proto__: null };
    this._eventsCount = 0;
  }
  this._maxListeners = this._maxListeners || undefined;
  if (opts && opts.captureRejections !== undefined) {
    if (typeof opts.captureRejections !== 'boolean') throw errors.invalidArgType('options.captureRejections', 'boolean', opts.captureRejections);
    this[kCapture] = Boolean(opts.captureRejections);
  } else {
    this[kCapture] = EventEmitter.prototype[kCapture];
  }
};
EventEmitter.prototype[kCapture] = false;

Object.defineProperty(EventEmitter, 'defaultMaxListeners', {
  enumerable: true,
  get: () => defaultMaxListeners,
  set(value) {
    if (typeof value !== 'number' || value < 0 || Number.isNaN(value)) throw errors.outOfRange('defaultMaxListeners', 'a non-negative number', value);
    defaultMaxListeners = value;
  },
});
Object.defineProperty(EventEmitter, 'captureRejections', {
  enumerable: true,
  get: () => EventEmitter.prototype[kCapture],
  set(value) {
    if (typeof value !== 'boolean') throw errors.invalidArgType('EventEmitter.captureRejections', 'boolean', value);
    EventEmitter.prototype[kCapture] = value;
  },
});
EventEmitter.captureRejectionSymbol = kRejection;
EventEmitter.errorMonitor = errorMonitor;

EventEmitter.prototype.setMaxListeners = function setMaxListeners(n) {
  if (typeof n !== 'number' || n < 0 || Number.isNaN(n)) throw errors.outOfRange('n', 'a non-negative number', n);
  this._maxListeners = n;
  return this;
};
const maxOf = (emitter) => (emitter._maxListeners === undefined ? defaultMaxListeners : emitter._maxListeners);
EventEmitter.prototype.getMaxListeners = function getMaxListeners() {
  return maxOf(this);
};

function addCatch(emitter, promise, type, args) {
  if (!emitter[kCapture]) return;
  try {
    const then = promise.then;
    if (typeof then === 'function') {
      then.call(promise, undefined, (err) => {
        // Defer so a throwing handler surfaces as an uncaught error, not a swallowed rejection.
        Promise.resolve().then(() => emitUnhandledRejectionOrErr(emitter, err, type, args));
      });
    }
  } catch (err) {
    emitter.emit('error', err);
  }
}
function emitUnhandledRejectionOrErr(emitter, err, type, args) {
  if (typeof emitter[kRejection] === 'function') emitter[kRejection](err, type, ...args);
  else {
    const prev = emitter[kCapture];
    try {
      emitter[kCapture] = false;
      emitter.emit('error', err);
    } finally {
      emitter[kCapture] = prev;
    }
  }
}

EventEmitter.prototype.emit = function emit(type, ...args) {
  let doError = type === 'error';
  const events = this._events;
  if (events !== undefined) {
    if (doError && events[errorMonitor] !== undefined) this.emit(errorMonitor, ...args);
    doError = doError && events.error === undefined;
  } else if (!doError) {
    return false;
  }
  if (doError) {
    let er;
    if (args.length > 0) er = args[0];
    if (er instanceof Error) throw er;
    let shown;
    try { shown = require('util').inspect(er); } catch { shown = String(er); }
    const err = errors.make(Error, 'ERR_UNHANDLED_ERROR', `Unhandled error. (${shown})`);
    err.context = er;
    throw err;
  }
  const handler = events[type];
  if (handler === undefined) return false;
  if (typeof handler === 'function') {
    const result = handler.apply(this, args);
    if (result !== undefined && result !== null) addCatch(this, result, type, args);
  } else {
    for (const listener of handler.slice()) {
      const result = listener.apply(this, args);
      if (result !== undefined && result !== null) addCatch(this, result, type, args);
    }
  }
  return true;
};

function addListener(target, type, listener, prepend) {
  errors.validateFunction(listener, 'listener');
  let events = target._events;
  let existing;
  if (events === undefined) {
    events = target._events = { __proto__: null };
    target._eventsCount = 0;
  } else {
    if (events.newListener !== undefined) {
      target.emit('newListener', type, listener.listener ? listener.listener : listener);
      events = target._events;
    }
    existing = events[type];
  }
  if (existing === undefined) {
    events[type] = listener;
    ++target._eventsCount;
  } else {
    if (typeof existing === 'function') existing = events[type] = prepend ? [listener, existing] : [existing, listener];
    else if (prepend) existing.unshift(listener);
    else existing.push(listener);
    const max = maxOf(target);
    if (max > 0 && existing.length > max && !existing.warned) {
      existing.warned = true;
      const warning = new Error(`Possible EventEmitter memory leak detected. ${existing.length} ${String(type)} listeners added to ${target.constructor ? target.constructor.name : 'EventEmitter'}. MaxListeners is ${max}. Use emitter.setMaxListeners() to increase limit`);
      warning.name = 'MaxListenersExceededWarning';
      warning.emitter = target;
      warning.type = type;
      warning.count = existing.length;
      const proc = globalThis.process;
      if (proc && typeof proc.emitWarning === 'function') proc.emitWarning(warning);
      else if (globalThis.console) globalThis.console.error(`${warning.name}: ${warning.message}`);
    }
  }
  return target;
}

EventEmitter.prototype.addListener = function addListenerMethod(type, listener) {
  return addListener(this, type, listener, false);
};
EventEmitter.prototype.on = EventEmitter.prototype.addListener;
EventEmitter.prototype.prependListener = function prependListener(type, listener) {
  return addListener(this, type, listener, true);
};

function onceWrap(target, type, listener) {
  const state = { fired: false, wrapFn: undefined, target, type, listener };
  const wrapped = function onceWrapper(...args) {
    if (!state.fired) {
      state.target.removeListener(state.type, state.wrapFn);
      state.fired = true;
      return state.listener.apply(state.target, args);
    }
    return undefined;
  };
  wrapped.listener = listener;
  state.wrapFn = wrapped;
  return wrapped;
}
EventEmitter.prototype.once = function once(type, listener) {
  errors.validateFunction(listener, 'listener');
  this.on(type, onceWrap(this, type, listener));
  return this;
};
EventEmitter.prototype.prependOnceListener = function prependOnceListener(type, listener) {
  errors.validateFunction(listener, 'listener');
  this.prependListener(type, onceWrap(this, type, listener));
  return this;
};

EventEmitter.prototype.removeListener = function removeListener(type, listener) {
  errors.validateFunction(listener, 'listener');
  const events = this._events;
  if (events === undefined) return this;
  const list = events[type];
  if (list === undefined) return this;
  if (list === listener || list.listener === listener) {
    this._eventsCount -= 1;
    if (this._eventsCount === 0) this._events = { __proto__: null };
    else {
      delete events[type];
      if (events.removeListener) this.emit('removeListener', type, list.listener || listener);
    }
  } else if (typeof list !== 'function') {
    let position = -1;
    for (let i = list.length - 1; i >= 0; i--) {
      if (list[i] === listener || list[i].listener === listener) { position = i; break; }
    }
    if (position < 0) return this;
    if (position === 0) list.shift(); else list.splice(position, 1);
    if (list.length === 1) events[type] = list[0];
    if (events.removeListener !== undefined) this.emit('removeListener', type, listener);
  }
  return this;
};
EventEmitter.prototype.off = EventEmitter.prototype.removeListener;

EventEmitter.prototype.removeAllListeners = function removeAllListeners(type) {
  const events = this._events;
  if (events === undefined) return this;
  if (events.removeListener === undefined) {
    if (arguments.length === 0) {
      this._events = { __proto__: null };
      this._eventsCount = 0;
    } else if (events[type] !== undefined) {
      if (--this._eventsCount === 0) this._events = { __proto__: null };
      else delete events[type];
    }
    return this;
  }
  if (arguments.length === 0) {
    for (const key of Reflect.ownKeys(events)) {
      if (key !== 'removeListener') this.removeAllListeners(key);
    }
    this.removeAllListeners('removeListener');
    this._events = { __proto__: null };
    this._eventsCount = 0;
    return this;
  }
  const listeners = events[type];
  if (typeof listeners === 'function') this.removeListener(type, listeners);
  else if (listeners !== undefined) {
    for (let i = listeners.length - 1; i >= 0; i--) this.removeListener(type, listeners[i]);
  }
  return this;
};

function listenersOf(target, type, unwrap) {
  const events = target._events;
  if (events === undefined) return [];
  const list = events[type];
  if (list === undefined) return [];
  if (typeof list === 'function') return unwrap ? [list.listener || list] : [list];
  return unwrap ? list.map((l) => l.listener || l) : list.slice();
}
EventEmitter.prototype.listeners = function listeners(type) { return listenersOf(this, type, true); };
EventEmitter.prototype.rawListeners = function rawListeners(type) { return listenersOf(this, type, false); };
EventEmitter.prototype.listenerCount = function listenerCount(type, listener) {
  const events = this._events;
  if (events === undefined) return 0;
  const list = events[type];
  if (typeof list === 'function') return listener == null || list === listener || list.listener === listener ? 1 : 0;
  if (list === undefined) return 0;
  if (listener == null) return list.length;
  return list.filter((l) => l === listener || l.listener === listener).length;
};
EventEmitter.prototype.eventNames = function eventNames() {
  return this._eventsCount > 0 ? Reflect.ownKeys(this._events) : [];
};

EventEmitter.listenerCount = (emitter, type) => emitter.listenerCount(type);

function isEventTarget(value) {
  return value !== null && typeof value === 'object' && typeof value.addEventListener === 'function' && typeof value.removeEventListener === 'function';
}
const abortError = (signal) => {
  const error = new Error('The operation was aborted', signal && signal.reason !== undefined ? { cause: signal.reason } : undefined);
  error.name = 'AbortError';
  error.code = 'ABORT_ERR';
  return error;
};

EventEmitter.once = function once(emitter, name, options = {}) {
  const signal = options && options.signal;
  return new Promise((resolve, reject) => {
    if (signal && signal.aborted) { reject(abortError(signal)); return; }
    const target = isEventTarget(emitter);
    if (!target && typeof emitter.on !== 'function') throw errors.invalidArgType('emitter', 'EventEmitter', emitter);
    let onAbort;
    const cleanup = () => {
      if (signal && onAbort) signal.removeEventListener('abort', onAbort);
    };
    let errorListener;
    const resolver = (...args) => {
      if (errorListener) emitter.removeListener('error', errorListener);
      cleanup();
      resolve(args);
    };
    if (target) emitter.addEventListener(name, resolver, { once: true });
    else {
      emitter.once(name, resolver);
      if (name !== 'error') {
        errorListener = (err) => {
          emitter.removeListener(name, resolver);
          cleanup();
          reject(err);
        };
        emitter.once('error', errorListener);
      }
    }
    if (signal) {
      onAbort = () => {
        if (target) emitter.removeEventListener(name, resolver); else {
          emitter.removeListener(name, resolver);
          if (errorListener) emitter.removeListener('error', errorListener);
        }
        reject(abortError(signal));
      };
      signal.addEventListener('abort', onAbort, { once: true });
    }
  });
};

EventEmitter.on = function on(emitter, event, options = {}) {
  const signal = options && options.signal;
  if (signal && signal.aborted) throw abortError(signal);
  const queue = [];
  const waiting = [];
  let error = null;
  let finished = false;
  const target = isEventTarget(emitter);
  const handler = (...args) => {
    const waiter = waiting.shift();
    if (waiter) waiter.resolve({ value: args, done: false }); else queue.push(args);
  };
  const errorHandler = (err) => {
    finished = true;
    const waiter = waiting.shift();
    if (waiter) waiter.reject(err); else error = err;
    iterator.return();
  };
  const abortListener = () => errorHandler(abortError(signal));
  const iterator = Object.setPrototypeOf({
    next() {
      const value = queue.shift();
      if (value) return Promise.resolve({ value, done: false });
      if (error) {
        const p = Promise.reject(error);
        error = null;
        return p;
      }
      if (finished) return Promise.resolve({ value: undefined, done: true });
      return new Promise((resolve, reject) => waiting.push({ resolve, reject }));
    },
    return() {
      if (target) emitter.removeEventListener(event, handler); else {
        emitter.removeListener(event, handler);
        emitter.removeListener('error', errorHandler);
      }
      if (signal) signal.removeEventListener('abort', abortListener);
      finished = true;
      for (const waiter of waiting.splice(0)) waiter.resolve({ value: undefined, done: true });
      return Promise.resolve({ value: undefined, done: true });
    },
    throw(err) {
      error = err;
      if (target) emitter.removeEventListener(event, handler); else {
        emitter.removeListener(event, handler);
        emitter.removeListener('error', errorHandler);
      }
      return Promise.reject(err);
    },
    [Symbol.asyncIterator]() { return this; },
  }, Object.getPrototypeOf(Object.getPrototypeOf(async function* () {}).prototype));
  if (target) emitter.addEventListener(event, handler);
  else {
    emitter.on(event, handler);
    if (event !== 'error') emitter.on('error', errorHandler);
  }
  if (signal) signal.addEventListener('abort', abortListener, { once: true });
  return iterator;
};

EventEmitter.getEventListeners = function getEventListeners(emitter, type) {
  if (typeof emitter.listeners === 'function') return emitter.listeners(type);
  throw errors.invalidArgType('emitter', ['EventEmitter', 'EventTarget'], emitter);
};
EventEmitter.getMaxListeners = function getMaxListeners(emitter) {
  if (typeof emitter.getMaxListeners === 'function') return maxOf(emitter);
  throw errors.invalidArgType('emitter', ['EventEmitter', 'EventTarget'], emitter);
};
EventEmitter.setMaxListeners = function setMaxListeners(n = defaultMaxListeners, ...targets) {
  if (typeof n !== 'number' || n < 0 || Number.isNaN(n)) throw errors.outOfRange('n', 'a non-negative number', n);
  if (targets.length === 0) defaultMaxListeners = n;
  else for (const target of targets) target.setMaxListeners(n);
};
EventEmitter.addAbortListener = function addAbortListener(signal, listener) {
  if (signal === undefined) throw errors.invalidArgType('signal', 'AbortSignal', signal);
  errors.validateFunction(listener, 'listener');
  let removeEventListener;
  if (signal.aborted) Promise.resolve().then(() => listener());
  else {
    signal.addEventListener('abort', listener, { once: true });
    removeEventListener = () => signal.removeEventListener('abort', listener);
  }
  return { [Symbol.dispose]() { if (removeEventListener) removeEventListener(); } };
};

EventEmitter.EventEmitter = EventEmitter;
module.exports = EventEmitter;

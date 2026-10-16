'use strict';
// node:diagnostics_channel: named channels, subscribe/publish, bindStore/runStores over
// AsyncLocalStorage and TracingChannel. Written for this runtime; no Node source is copied.
//
// shortcut: channels live in a plain Map for the life of the process (Node holds them weakly
// while nobody subscribes), fine for the handful of well-known names; switch to WeakRef when a
// program creates channels with unbounded distinct names.
const errors = require('internal/errors');
const process = require('process');

const channels = new Map();

// A subscriber or transform that throws must not break the publisher or its peers: the error
// is rethrown from a tick, where it reaches `uncaughtException` like Node's.
const rethrowLater = (error) => process.nextTick(() => { throw error; });

function validateName(name) {
  if (typeof name !== 'string' && typeof name !== 'symbol') throw errors.invalidArgType('channel', 'string or symbol', name);
}

function wrapStoreRun(store, data, next, transform) {
  return () => {
    let context;
    try {
      context = transform ? transform(data) : data;
    } catch (error) {
      rethrowLater(error);
      return next();
    }
    return store.run(context, next);
  };
}

class Channel {
  constructor(name) {
    this.name = name;
    this._subscribers = [];
    this._stores = new Map();
  }

  get hasSubscribers() { return this._subscribers.length > 0 || this._stores.size > 0; }

  subscribe(subscription) {
    errors.validateFunction(subscription, 'subscription');
    this._subscribers = [...this._subscribers, subscription];
  }

  unsubscribe(subscription) {
    const index = this._subscribers.indexOf(subscription);
    if (index === -1) return false;
    this._subscribers = this._subscribers.filter((_, i) => i !== index);
    return true;
  }

  bindStore(store, transform) {
    if (store === null || typeof store !== 'object' || typeof store.run !== 'function') {
      throw errors.invalidArgType('store', 'AsyncLocalStorage', store);
    }
    this._stores.set(store, transform);
  }

  unbindStore(store) {
    return this._stores.delete(store);
  }

  publish(data) {
    // `_subscribers` is replaced, never mutated: a subscriber may unsubscribe while we iterate.
    for (const subscriber of this._subscribers) {
      try {
        subscriber(data, this.name);
      } catch (error) {
        rethrowLater(error);
      }
    }
  }

  runStores(data, fn, thisArg, ...args) {
    let run = () => {
      this.publish(data);
      return Reflect.apply(fn, thisArg, args);
    };
    for (const [store, transform] of this._stores) run = wrapStoreRun(store, data, run, transform);
    return run();
  }
}

function channel(name) {
  validateName(name);
  let found = channels.get(name);
  if (found === undefined) {
    found = new Channel(name);
    channels.set(name, found);
  }
  return found;
}

const hasSubscribers = (name) => {
  validateName(name);
  const found = channels.get(name);
  return found !== undefined && found.hasSubscribers;
};
const subscribe = (name, subscription) => channel(name).subscribe(subscription);
const unsubscribe = (name, subscription) => channel(name).unsubscribe(subscription);

const TRACE_EVENTS = ['start', 'end', 'asyncStart', 'asyncEnd', 'error'];

class TracingChannel {
  constructor(nameOrChannels) {
    if (typeof nameOrChannels === 'string') {
      for (const event of TRACE_EVENTS) this[event] = channel(`tracing:${nameOrChannels}:${event}`);
      return;
    }
    if (nameOrChannels === null || typeof nameOrChannels !== 'object') {
      throw errors.invalidArgType('nameOrChannels', 'string or object', nameOrChannels);
    }
    for (const event of TRACE_EVENTS) {
      if (!(nameOrChannels[event] instanceof Channel)) {
        throw errors.invalidArgType(`nameOrChannels.${event}`, 'Channel', nameOrChannels[event]);
      }
      this[event] = nameOrChannels[event];
    }
  }

  get hasSubscribers() { return TRACE_EVENTS.some((event) => this[event].hasSubscribers); }

  subscribe(handlers) {
    for (const event of TRACE_EVENTS) if (handlers[event]) this[event].subscribe(handlers[event]);
  }

  unsubscribe(handlers) {
    let done = true;
    for (const event of TRACE_EVENTS) {
      if (handlers[event] && !this[event].unsubscribe(handlers[event])) done = false;
    }
    return done;
  }

  traceSync(fn, context = {}, thisArg, ...args) {
    if (!this.hasSubscribers) return Reflect.apply(fn, thisArg, args);
    const { start, end, error } = this;
    return start.runStores(context, () => {
      try {
        const result = Reflect.apply(fn, thisArg, args);
        context.result = result;
        return result;
      } catch (err) {
        context.error = err;
        error.publish(context);
        throw err;
      } finally {
        end.publish(context);
      }
    });
  }

  tracePromise(fn, context = {}, thisArg, ...args) {
    if (!this.hasSubscribers) return Reflect.apply(fn, thisArg, args);
    const { start, end, asyncStart, asyncEnd, error } = this;
    const settle = () => asyncStart.runStores(context, () => asyncEnd.publish(context));
    return start.runStores(context, () => {
      try {
        return Promise.resolve(Reflect.apply(fn, thisArg, args)).then(
          (result) => {
            context.result = result;
            settle();
            return result;
          },
          (err) => {
            context.error = err;
            error.publish(context);
            settle();
            throw err;
          },
        );
      } catch (err) {
        context.error = err;
        error.publish(context);
        throw err;
      } finally {
        end.publish(context);
      }
    });
  }

  traceCallback(fn, position = -1, context = {}, thisArg, ...args) {
    if (!this.hasSubscribers) return Reflect.apply(fn, thisArg, args);
    const { start, end, asyncStart, asyncEnd, error } = this;
    const index = position < 0 ? args.length + position : position;
    const callback = args[index];
    errors.validateFunction(callback, 'callback');
    args[index] = function wrapped(err, result) {
      if (err) {
        context.error = err;
        error.publish(context);
      } else {
        context.result = result;
      }
      return asyncStart.runStores(context, () => {
        try {
          return Reflect.apply(callback, this, arguments);
        } finally {
          asyncEnd.publish(context);
        }
      });
    };
    return start.runStores(context, () => {
      try {
        return Reflect.apply(fn, thisArg, args);
      } catch (err) {
        context.error = err;
        error.publish(context);
        throw err;
      } finally {
        end.publish(context);
      }
    });
  }
}

module.exports = {
  Channel,
  channel,
  hasSubscribers,
  subscribe,
  tracingChannel: (nameOrChannels) => new TracingChannel(nameOrChannels),
  unsubscribe,
};

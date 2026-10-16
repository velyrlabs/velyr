'use strict';
// node:timers and node:timers/promises. Written for this runtime; no Node source is copied.
// setTimeout/setInterval return Timeout objects (ref/unref/hasRef/refresh/close and a numeric
// toPrimitive) on top of the host's number-returning timers; setImmediate comes from
// internal/timers_immediate.
//
// ref()/unref() drive the web-core timer ref hook (Symbol.for('velyr.timers')), so an unref'd
// timer does not keep the event loop alive.
const errors = require('internal/errors');
const immediates = require('internal/timers_immediate');

const TIMEOUT_MAX = 2 ** 31 - 1;
const global = globalThis;
// Captured once, before install_process_global replaces the globals with the wrappers below.
const native = {
  setTimeout: global.setTimeout, clearTimeout: global.clearTimeout,
  setInterval: global.setInterval, clearInterval: global.clearInterval,
};
const hostTimer = (name) => {
  if (typeof native[name] !== 'function') throw new Error(`velyr compat-node: the host did not install ${name}`);
  return native[name];
};

// web-core's ref hook; absent on hosts without it, where unref() degrades to recording the flag.
const hostRef = (handle, refd) => global[Symbol.for('velyr.timers')]?.ref(handle, refd);

const active = new Map();
let nextId = 1;

class Timeout {
  #id = nextId++;
  #handle;
  #refed = true;
  #repeat;
  #callback;
  #args;

  constructor(callback, delay, args, repeat) {
    errors.validateFunction(callback, 'callback');
    delay *= 1;
    if (!(delay >= 1 && delay <= TIMEOUT_MAX)) delay = 1;
    this._idleTimeout = delay;
    this._onTimeout = callback;
    this._repeat = repeat ? delay : null;
    this.#repeat = repeat;
    this.#callback = callback;
    this.#args = args;
    this.#arm();
  }

  #arm() {
    const fire = () => {
      if (!this.#repeat) active.delete(this.#id);
      this.#callback(...this.#args);
    };
    this.#handle = (this.#repeat ? hostTimer('setInterval') : hostTimer('setTimeout'))(fire, this._idleTimeout);
    active.set(this.#id, this);
    if (!this.#refed) hostRef(this.#handle, false);
  }

  hasRef() { return this.#refed; }
  ref() { this.#refed = true; if (active.has(this.#id)) hostRef(this.#handle, true); return this; }
  unref() { this.#refed = false; if (active.has(this.#id)) hostRef(this.#handle, false); return this; }

  refresh() {
    this.close();
    this.#arm();
    return this;
  }

  close() {
    if (active.delete(this.#id)) {
      (this.#repeat ? hostTimer('clearInterval') : hostTimer('clearTimeout'))(this.#handle);
    }
    return this;
  }

  [Symbol.toPrimitive]() { return this.#id; }
  [Symbol.dispose]() { this.close(); }
}

function setTimeout(callback, delay, ...args) {
  return new Timeout(callback, delay, args, false);
}
setTimeout[Symbol.for('nodejs.util.promisify.custom')] = (delay, value) => timersPromises().setTimeout(delay, value);

function setInterval(callback, delay, ...args) {
  return new Timeout(callback, delay, args, true);
}

function clear(timer) {
  if (timer instanceof Timeout) timer.close();
  else if (typeof timer === 'number' || typeof timer === 'string') active.get(Number(timer))?.close();
}

let promisesModule;
const timersPromises = () => (promisesModule ??= require('timers/promises'));

const exports_ = {
  setTimeout, clearTimeout: clear, setInterval, clearInterval: clear,
  setImmediate: immediates.setImmediate, clearImmediate: immediates.clearImmediate,
};
Object.defineProperty(exports_, 'promises', { enumerable: true, configurable: true, get: timersPromises });
Object.defineProperty(exports_, Symbol.for('velyr.immediate.dispatch'), {
  value: immediates[Symbol.for('velyr.immediate.dispatch')],
});
module.exports = exports_;

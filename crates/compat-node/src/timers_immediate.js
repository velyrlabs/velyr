'use strict';
// node:timers: setImmediate/clearImmediate (check phase, after I/O and before timers) and the
// timer functions the host installed on the global object. Written for this runtime.
const errors = require('internal/errors');
const ops = require('internal/ops');
const process = require('process');

const pending = new Map();

class Immediate {
  #id;
  #refed = true;
  constructor(id, callback, args) {
    this.#id = id;
    this._onImmediate = callback;
    this._argv = args;
    pending.set(id, this);
  }
  hasRef() { return this.#refed; }
  ref() { if (pending.has(this.#id)) { this.#refed = true; ops.immediateRef(this.#id, true); } return this; }
  unref() { if (pending.has(this.#id)) { this.#refed = false; ops.immediateRef(this.#id, false); } return this; }
  _clear() { if (pending.delete(this.#id)) ops.immediateClear(this.#id); }
  [Symbol.dispose]() { this._clear(); }
}

function setImmediate(callback, ...args) {
  errors.validateFunction(callback, 'callback');
  return new Immediate(ops.immediateSet(), callback, args);
}
setImmediate[Symbol.for('nodejs.util.promisify.custom')] = (value) =>
  new Promise((resolve) => setImmediate(resolve, value));

function clearImmediate(immediate) {
  if (immediate instanceof Immediate) immediate._clear();
}

// The loop calls this with an immediate id; a throwing callback goes the uncaught route.
function dispatch(id) {
  const immediate = pending.get(id);
  if (!immediate) return;
  pending.delete(id);
  try { immediate._onImmediate(...immediate._argv); } catch (error) { process._uncaught(error); }
}

const global = globalThis;
for (const name of ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval']) {
  Object.defineProperty(exports, name, { enumerable: true, configurable: true, get: () => global[name] });
}
exports.setImmediate = setImmediate;
exports.clearImmediate = clearImmediate;
Object.defineProperty(exports, Symbol.for('velyr.immediate.dispatch'), { value: dispatch });

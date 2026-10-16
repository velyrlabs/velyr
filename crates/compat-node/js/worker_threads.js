'use strict';
// node:worker_threads over the web `Worker`, `MessageChannel`, `MessagePort` and
// `BroadcastChannel` of runtime-workers. Written for this runtime.
//
// Permissions are the web Worker's: `parent ∩ requested ∩ deployment policy`, where `requested` is
// `options.velyr.permissions` (a Velyr extension; absent means "what the parent has").
//
// workerData: a worker reads it while its entry module loads, before any message can arrive, so the
// parent structured-clones `{ workerData, environmentData }` into a host table (`op_node_worker`)
// and passes the key in the web Worker `name`; the worker takes it once. A worker that never loads
// this module still sees the raw name.
//
// Exit: runtime-workers sends an internal 'ended' event (exit code) after the worker's last message
// and uncaught error. The code is what the worker asked for (`process.exit(n)`, 0 for `close()`),
// 1 after an error, a quota stop or terminate(), otherwise 0.
//
// resourceLimits: only `maxOldGenerationSizeMb`, mapped to the worker's heap limit (`velyr.heapLimitMb`
// of the web Worker; the host caps it at the parent's). A worker that hits it emits an `error` with
// code ERR_WORKER_OUT_OF_MEMORY, then 'exit' 1.
//
// Known limits (each throws ERR_VELYR_UNSUPPORTED): `eval: true`, `env` (SHARE_ENV or an object),
// the other `resourceLimits` keys, `argv`, `execArgv`, `stdin`/`stdout`/`stderr` options,
// `transferList` for workerData, `worker.ref()`, `receiveMessageOnPort`. Also not provided:
// 'online' event, `worker.stdout`/`stderr`/`stdin`, `worker.performance`. After `worker.unref()` a
// natural exit is only reported while something else keeps the event loop alive.
const errors = require('internal/errors');
const EventEmitter = require('events');
const ops = require('internal/ops');
const { pathToFileURL } = require('url');

const NAME_PREFIX = '\u0000velyr:wt:';
const SHARE_ENV = Symbol.for('nodejs.worker_threads.SHARE_ENV');

// Captured now so later changes to the globals cannot redirect this module.
const WebWorker = globalThis.Worker;
const WebMessagePort = globalThis.MessagePort;
const WebMessageChannel = globalThis.MessageChannel;
const WebBroadcastChannel = globalThis.BroadcastChannel;
const unrefWorker = globalThis.Velyr && globalThis.Velyr.unrefWorker;

function unsupported(what) {
  return errors.make(Error, 'ERR_VELYR_UNSUPPORTED', `${what} is not supported by this runtime`);
}

const environmentData = new Map();

// Node-style `on`/`once`/`off` over `addEventListener`: 'message' listeners get the data, 'close'
// gets nothing.
function nodeEventMethods() {
  const table = new WeakMap();
  const listenersOf = (self, type) => {
    let types = table.get(self);
    if (types === undefined) table.set(self, (types = new Map()));
    let byListener = types.get(type);
    if (byListener === undefined) types.set(type, (byListener = new Map()));
    return byListener;
  };
  function remove(self, type, listener) {
    const byListener = listenersOf(self, type);
    const wrapper = byListener.get(listener);
    if (wrapper === undefined) return;
    byListener.delete(listener);
    self.removeEventListener(type, wrapper);
  }
  function add(self, type, listener, once) {
    errors.validateFunction(listener, 'listener');
    const byListener = listenersOf(self, type);
    if (byListener.has(listener)) return self;
    const wrapper = (event) => {
      if (once) remove(self, type, listener);
      const args = type === 'message' ? [event.data] : type === 'close' ? [] : [event];
      Reflect.apply(listener, self, args);
    };
    byListener.set(listener, wrapper);
    self.addEventListener(type, wrapper);
    // Only `onmessage` starts a web port; a Node `on('message')` must too.
    if (type === 'message') self.start();
    return self;
  }
  return {
    on(type, listener) { return add(this, type, listener, false); },
    addListener(type, listener) { return add(this, type, listener, false); },
    once(type, listener) { return add(this, type, listener, true); },
    off(type, listener) { remove(this, type, listener); return this; },
    removeListener(type, listener) { remove(this, type, listener); return this; },
  };
}

// Node's MessagePort is an EventEmitter too; give the web one the same methods, if it lacks them.
if (typeof WebMessagePort === 'function') {
  const methods = nodeEventMethods();
  for (const key of Object.keys(methods)) {
    if (!(key in WebMessagePort.prototype)) {
      Object.defineProperty(WebMessagePort.prototype, key, { value: methods[key], writable: true, configurable: true });
    }
  }
}

let isMainThread = true;
let threadId = 0;
let workerData;
let resourceLimits = {};
let parentPort = null;

// A worker scope is the only isolate with `postMessage` and `close` as globals.
if (typeof globalThis.postMessage === 'function' && typeof globalThis.close === 'function') {
  isMainThread = false;
  const scope = {
    postMessage: globalThis.postMessage,
    close: globalThis.close,
    addEventListener: globalThis.addEventListener,
    removeEventListener: globalThis.removeEventListener,
  };
  const name = String(globalThis.name === undefined ? '' : globalThis.name);
  const end = name.startsWith(NAME_PREFIX) ? name.indexOf('\u0000', NAME_PREFIX.length) : -1;
  if (end !== -1) {
    const taken = ops.worker('take', name.slice(NAME_PREFIX.length, end));
    Object.defineProperty(globalThis, 'name', { value: name.slice(end + 1), writable: true, configurable: true, enumerable: false });
    if (taken !== undefined) {
      threadId = taken[0];
      workerData = taken[1].workerData;
      resourceLimits = taken[1].resourceLimits;
      for (const [key, value] of taken[1].environmentData) environmentData.set(key, value);
    }
  }
  class ParentPort {
    postMessage(value, transferList) { Reflect.apply(scope.postMessage, globalThis, [value, transferList]); }
    close() { Reflect.apply(scope.close, globalThis, []); }
    start() {}
    addEventListener(type, listener, options) { Reflect.apply(scope.addEventListener, globalThis, [type, listener, options]); }
    removeEventListener(type, listener, options) { Reflect.apply(scope.removeEventListener, globalThis, [type, listener, options]); }
    get [Symbol.toStringTag]() { return 'MessagePort'; }
  }
  const methods = nodeEventMethods();
  for (const key of Object.keys(methods)) {
    Object.defineProperty(ParentPort.prototype, key, { value: methods[key], writable: true, configurable: true });
  }
  parentPort = new ParentPort();
}

function toFileUrl(filename) {
  if (filename instanceof URL) {
    if (filename.protocol !== 'file:') throw errors.invalidArgValue('filename', filename.href, 'must be a file: URL');
    return filename;
  }
  errors.validateString(filename, 'filename');
  if (filename.startsWith('file:')) return new URL(filename);
  if (/^[a-zA-Z][a-zA-Z0-9+.-]+:/.test(filename)) throw errors.invalidArgValue('filename', filename, 'must be a path or a file: URL');
  if (!filename.startsWith('/') && !filename.startsWith('./') && !filename.startsWith('../')) {
    throw errors.make(TypeError, 'ERR_WORKER_PATH', `The worker script or module filename must be an absolute path or a relative path starting with './' or '../'. Received "${filename}"`);
  }
  return pathToFileURL(filename);
}

function checkResourceLimits(limits) {
  if (limits === undefined) return;
  errors.validateObject(limits, 'options.resourceLimits');
  for (const key of Object.keys(limits)) {
    if (key !== 'maxOldGenerationSizeMb') throw unsupported(`options.resourceLimits.${key}`);
  }
  const mb = limits.maxOldGenerationSizeMb;
  if (mb !== undefined && !(Number.isFinite(mb) && mb >= 1)) {
    throw errors.invalidArgValue('options.resourceLimits.maxOldGenerationSizeMb', mb, 'must be a number of at least 1');
  }
}

function outOfMemory() {
  return errors.make(Error, 'ERR_WORKER_OUT_OF_MEMORY', 'Worker terminated due to reaching memory limit: JS heap out of memory');
}

function checkOptions(options) {
  if (options === undefined || options === null) return {};
  errors.validateObject(options, 'options');
  if (options.eval) throw unsupported('options.eval (running a string as the worker)');
  if (options.env !== undefined) throw unsupported(options.env === SHARE_ENV ? 'SHARE_ENV' : 'options.env');
  checkResourceLimits(options.resourceLimits);
  if (options.argv !== undefined && options.argv.length > 0) throw unsupported('options.argv');
  if (options.execArgv !== undefined && options.execArgv.length > 0) throw unsupported('options.execArgv');
  for (const stream of ['stdin', 'stdout', 'stderr']) {
    if (options[stream]) throw unsupported(`options.${stream}`);
  }
  if (options.transferList !== undefined && options.transferList.length > 0) throw unsupported('options.transferList');
  return options;
}

class Worker extends EventEmitter {
  #worker;
  #key;
  #exited = false;
  #exitCode = 0;
  #terminating = false;
  #threadId;

  constructor(filename, options) {
    super();
    if (typeof WebWorker !== 'function') throw unsupported('Worker (the workers host service is not installed)');
    options = checkOptions(options);
    const url = toFileUrl(filename);
    const [id, key] = ops.worker('park', {
      workerData: options.workerData,
      environmentData: [...environmentData],
      resourceLimits: options.resourceLimits ?? {},
    });
    this.#threadId = id;
    this.#key = key;
    try {
      this.#worker = new WebWorker(url.href, {
        type: 'module',
        name: `${NAME_PREFIX}${key}\u0000${options.name === undefined ? '' : String(options.name)}`,
        velyr: { ...options.velyr, heapLimitMb: options.resourceLimits?.maxOldGenerationSizeMb },
      });
    } catch (error) {
      ops.worker('discard', key);
      throw error;
    }
    this.#worker.addEventListener('message', (event) => this.emit('message', event.data));
    this.#worker.addEventListener('messageerror', (event) => this.emit('messageerror', event));
    this.#worker.addEventListener('error', (event) => {
      event.preventDefault();
      const { error } = event;
      this.emit('error', error?.quota === 'heap' ? outOfMemory() : error);
    });
    // After any 'error', so a handler that awaits the error is back before 'exit' fires.
    this.#worker.addEventListener('ended', (event) => this.#finish(event.code));
  }

  get threadId() { return this.#threadId; }

  postMessage(value, transferList) {
    this.#worker.postMessage(value, transferList);
  }

  terminate() {
    if (this.#exited) return Promise.resolve(this.#exitCode);
    const exit = new Promise((resolve) => this.once('exit', resolve));
    if (!this.#terminating) {
      this.#terminating = true;
      this.#worker.terminate();
      // terminate() returns once the worker's thread has stopped (or the wait gave up).
      queueMicrotask(() => this.#finish(1));
    }
    return exit;
  }

  ref() { throw unsupported('worker.ref()'); }

  unref() {
    if (this.#exited) return;
    unrefWorker(this.#worker);
  }

  #finish(code) {
    if (this.#exited) return;
    this.#exited = true;
    this.#exitCode = code;
    ops.worker('discard', this.#key);
    this.emit('exit', code);
  }
}

function getEnvironmentData(key) {
  return environmentData.get(key);
}

function setEnvironmentData(key, value) {
  if (value === undefined) environmentData.delete(key);
  else environmentData.set(key, value);
}

// shortcut: Node makes structured clone throw DataCloneError for a marked object; the engine's
// serializer has no hook for that, so this only accepts the call (undici's webidl needs it).
// Upgrade when the engine can refuse marked objects.
function markAsUncloneable() {}

function receiveMessageOnPort() {
  throw unsupported('receiveMessageOnPort');
}

module.exports = {
  BroadcastChannel: WebBroadcastChannel,
  MessageChannel: WebMessageChannel,
  MessagePort: WebMessagePort,
  SHARE_ENV,
  Worker,
  getEnvironmentData,
  isMainThread,
  markAsUncloneable,
  parentPort,
  receiveMessageOnPort,
  resourceLimits,
  setEnvironmentData,
  threadId,
  workerData,
};

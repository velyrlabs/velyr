'use strict';
// node:process. Written for this runtime; no Node source is copied.
//
// Known limits: `process.env` writes live in the isolate (never the OS environment, never
// child processes); `exit` needs the host's `on_exit` callback to stop the isolate and then
// unwinds JS with a `ProcessExit` error; `beforeExit`, `exit`, `unhandledRejection` and the
// signal events are emitted by the host (`emit_*` helpers), not wired to OS signals;
// stdin has no data source (it reads as empty); `heapUsed`/`heapTotal` are 0;
// `cwd`/`chdir` are process-wide; there is no `ppid`; uptime counts from the first load.
const EventEmitter = require('events');
const errors = require('internal/errors');
const ops = require('internal/ops');

const NODE_VERSION = '24.11.0';
const defaults = { platform: 'linux', arch: 'x64', pid: 0, execPath: '', args: [], velyr: '0.0.0' };
let info = defaults;
try { info = ops.processInfo(); } catch { /* host without process ops: keep defaults */ }

const process = new EventEmitter();
Object.defineProperty(process, Symbol.toStringTag, { value: 'process', configurable: true });
const define = (name, value) => Object.defineProperty(process, name, { value, writable: true, configurable: true, enumerable: false });

// ---- queues ---------------------------------------------------------------------------------
let queue = [];
let draining = false;

function uncaught(error) {
  if (error && error.code === 'VELYR_PROCESS_EXIT') throw error;
  if (process.listenerCount('uncaughtException') > 0) {
    process.emit('uncaughtException', error, 'uncaughtException');
    return;
  }
  // Surfaces through the runtime's unhandled-rejection path with a readable string.
  Promise.reject(String((error && error.stack) || error));
}

function nextTick(callback, ...args) {
  errors.validateFunction(callback, 'callback');
  queue.push({ callback, args });
}

// Drains the whole queue, including ticks queued while draining; true when something ran.
function tickCallback() {
  if (draining || queue.length === 0) return false;
  draining = true;
  try {
    for (let i = 0; i < queue.length; i++) {
      const { callback, args } = queue[i];
      queue[i] = undefined;
      try { callback(...args); } catch (error) { uncaught(error); }
    }
  } finally {
    queue = [];
    draining = false;
  }
  return true;
}

// ---- env ------------------------------------------------------------------------------------
const deleted = new Set();
const toKey = (key) => typeof key === 'symbol' ? undefined : String(key);
const env = new Proxy(Object.create(null), {
  get(_, key) {
    const name = toKey(key);
    if (name === undefined) return undefined;
    const value = ops.envGet(name); // throws PermissionDenied when not granted
    return deleted.has(name) ? undefined : value;
  },
  set(_, key, value) {
    const name = toKey(key);
    if (name === undefined) throw new TypeError('Cannot convert a Symbol value to a string');
    ops.envSet(name, String(value));
    deleted.delete(name);
    return true;
  },
  has(_, key) {
    const name = toKey(key);
    if (name === undefined) return false;
    const present = ops.envHas(name);
    return present && !deleted.has(name);
  },
  deleteProperty(_, key) {
    const name = toKey(key);
    if (name === undefined) return true;
    ops.envDelete(name);
    deleted.add(name);
    return true;
  },
  ownKeys() { return ops.envKeys().filter((name) => !deleted.has(name)); },
  getOwnPropertyDescriptor(target, key) {
    const name = toKey(key);
    if (name === undefined) return undefined;
    let value;
    try { value = ops.envGet(name); } catch { return undefined; }
    if (value === undefined || deleted.has(name)) return undefined;
    return { value, writable: true, enumerable: true, configurable: true };
  },
  defineProperty(_, key, descriptor) {
    if (!('value' in descriptor)) throw errors.invalidArgValue('descriptor', descriptor, "must have a 'value' field");
    const name = toKey(key);
    if (name === undefined) throw new TypeError('Cannot convert a Symbol value to a string');
    ops.envSet(name, String(descriptor.value));
    deleted.delete(name);
    return true;
  },
});

// ---- exit -----------------------------------------------------------------------------------
let exitCode;
let exiting = false;
function validateExitCode(code) {
  if (code === undefined || code === null) return undefined;
  if (typeof code === 'string' && /^\d+$/.test(code)) return Number(code);
  if (!Number.isInteger(code)) throw errors.invalidArgType('code', 'number', code);
  return code;
}
function exit(code) {
  if (code !== undefined) process.exitCode = code;
  const final = exitCode === undefined ? 0 : exitCode;
  if (!exiting) {
    exiting = true;
    process.emit('exit', final);
  }
  ops.exit(final);
  const error = new Error(`process.exit(${final})`);
  error.name = 'ProcessExit';
  error.code = 'VELYR_PROCESS_EXIT';
  error.exitCode = final;
  throw error;
}

// ---- time / memory --------------------------------------------------------------------------
function hrtime(previous) {
  const [sec, nsec] = ops.hrtime();
  if (previous === undefined) return [sec, nsec];
  if (!Array.isArray(previous) || previous.length !== 2) throw errors.invalidArgType('time', 'Array', previous);
  let ds = sec - previous[0];
  let dn = nsec - previous[1];
  if (dn < 0) { ds -= 1; dn += 1e9; }
  return [ds, dn];
}
hrtime.bigint = () => { const [sec, nsec] = ops.hrtime(); return BigInt(sec) * 1000000000n + BigInt(nsec); };
function memoryUsage() {
  const [rss, heapTotal, heapUsed] = ops.memory();
  return { rss, heapTotal, heapUsed, external: 0, arrayBuffers: 0 };
}
memoryUsage.rss = () => ops.memory()[0];

// ---- warnings -------------------------------------------------------------------------------
function emitWarning(warning, type, code) {
  let options = {};
  if (type !== null && typeof type === 'object' && !Array.isArray(type)) options = type;
  else if (typeof type === 'string') options = { type, code };
  else if (typeof type === 'function') options = {};
  if (typeof warning === 'string') {
    const text = warning;
    warning = new Error(text);
    warning.name = String(options.type || 'Warning');
    if (options.code !== undefined) warning.code = options.code;
    if (options.detail !== undefined) warning.detail = options.detail;
  } else if (!(warning instanceof Error)) {
    throw errors.invalidArgType('warning', 'string or an instance of Error', warning);
  }
  if (warning.name === 'DeprecationWarning' && process.noDeprecation) return;
  nextTick(() => process.emit('warning', warning));
}
process.on('warning', (warning) => {
  const code = warning.code ? `[${warning.code}] ` : '';
  let line = `(node:${info.pid}) ${code}${warning.name}: ${warning.message}\n`;
  if (warning.detail) line += `${warning.detail}\n`;
  ops.write(2, line);
});

// ---- stdio ----------------------------------------------------------------------------------
// Real streams, built on first access (node:stream is large and requires this module).
function lazyStdio(name, create) {
  let stream;
  Object.defineProperty(process, name, {
    enumerable: true,
    configurable: true,
    get() { return stream || (stream = create()); },
    set(value) { stream = value; },
  });
}
function makeWritable(fd) {
  const { Writable } = require('stream');
  const stream = new Writable({
    decodeStrings: false,
    write(chunk, encoding, callback) {
      try { ops.write(fd, chunk); } catch (error) { return callback(error); }
      callback();
    },
  });
  stream.fd = fd;
  stream.isTTY = false;
  stream._isStdio = true;
  return stream;
}
function makeStdin() {
  const { Readable } = require('stream');
  // No data source behind fd 0 yet: it reads as an empty, already finished input.
  const stream = new Readable({ read() { this.push(null); } });
  stream.fd = 0;
  stream.isTTY = false;
  return stream;
}

// ---- cwd ------------------------------------------------------------------------------------
function chdir(directory) {
  errors.validateString(directory, 'directory');
  const failure = ops.chdir(directory);
  if (failure !== undefined) {
    const error = new Error(`${failure}: no such file or directory, chdir ${process.cwd()} -> '${directory}'`);
    error.code = failure;
    error.syscall = 'chdir';
    error.path = process.cwd();
    error.dest = directory;
    throw error;
  }
}

// ---- assemble (enumerable own keys are the public exports) ----------------------------------
const args = info.args;
Object.assign(process, {
  title: 'node',
  version: `v${NODE_VERSION}`,
  versions: { node: NODE_VERSION, velyr: info.velyr },
  arch: info.arch,
  platform: info.platform,
  release: { name: 'node' },
  argv: [info.execPath, ...(info.script === undefined ? [] : [info.script]), ...args],
  execArgv: [],
  env,
  pid: info.pid,
  execPath: info.execPath,
  noDeprecation: false,
  exit,
  cwd: () => ops.cwd(),
  chdir,
  hrtime,
  memoryUsage,
  uptime: () => ops.uptime(),
  nextTick,
  emitWarning,
});
process.argv0 = 'node';
Object.defineProperty(process, 'exitCode', {
  enumerable: true,
  configurable: true,
  get: () => exitCode,
  set(value) { exitCode = validateExitCode(value); },
});
lazyStdio('stdout', () => makeWritable(1));
lazyStdio('stderr', () => makeWritable(2));
lazyStdio('stdin', makeStdin);
define('_tickCallback', tickCallback);
define('_uncaught', uncaught);
module.exports = process;

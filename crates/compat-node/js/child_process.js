'use strict';
// node:child_process over Velyr.Command (io-process) for the async API, and one host op for the
// synchronous one. Written for this runtime.
//
// Security model: nothing here decides what may run. Every spawn goes through io-process' gate
// (ProcessSpawn by program name or resolved path, `deny` wins, the resolved path is what runs), so
// without a grant `spawn` fails with ERR_ACCESS_DENIED. There is no implicit shell: `exec`,
// `execSync` and `{ shell }` run `/bin/sh -c`, so they need ProcessSpawn for the shell itself. The
// child gets exactly `options.env` (default: this process' `process.env`, which already honors the
// env permission); nothing else is inherited.
//
// Known limits (each throws ERR_VELYR_UNSUPPORTED): signals other than SIGKILL (`kill()` with no
// argument sends SIGKILL, not SIGTERM, and a `timeout` kills with SIGKILL), `detached`, `uid`/`gid`,
// `fork()` (see below), stdio other than pipe/inherit/ignore. `LD_*` and `DYLD_*` variables in a
// child's environment, explicit or inherited from `process.env`, are refused (ERR_ACCESS_DENIED).
// `maxBuffer` is at most 64 MiB per stream (ERR_OUT_OF_RANGE above that). Async `maxBuffer` is
// counted in JS on the chunks the pipe delivers (memory stays under `maxBuffer` plus one 16 KiB
// chunk); the sync API enforces it in Rust.
const errors = require('internal/errors');
const process = require('process');
const EventEmitter = require('events');
const { Buffer } = require('buffer');
const { Readable, Writable } = require('stream');
const ops = require('internal/ops');

const DEFAULT_MAX_BUFFER = 1024 * 1024;
const MAX_BUFFER_LIMIT = 64 * 1024 * 1024; // io-process' ceiling per stream; asking for more is an error
const HIGH_WATER_MARK = 64 * 1024;
const MODES = { pipe: 'piped', overlapped: 'piped', inherit: 'inherit', ignore: 'null' };
const SIGNALS = { 1: 'SIGHUP', 2: 'SIGINT', 3: 'SIGQUIT', 6: 'SIGABRT', 9: 'SIGKILL', 13: 'SIGPIPE', 14: 'SIGALRM', 15: 'SIGTERM' };
const kCustomPromisify = Symbol.for('nodejs.util.promisify.custom');

function unsupported(what) {
  return errors.make(Error, 'ERR_VELYR_UNSUPPORTED', `${what} is not supported by this runtime`);
}

function velyr() {
  const api = globalThis.Velyr;
  if (typeof api?.Command !== 'function') throw new Error('velyr node:child_process: install the process host service (velyr-io-process) first');
  return api;
}

function tag(error, props) {
  return Object.assign(error, props);
}

/** Node-style error for what io-process throws. */
function translate(error, file, args) {
  if (error?.code === 'VELYR_PERMISSION_DENIED') {
    const denied = errors.make(Error, 'ERR_ACCESS_DENIED', error.message);
    Object.defineProperty(denied, 'cause', { value: error, writable: true, configurable: true });
    return denied;
  }
  if (error?.code === 'VELYR_NOT_FOUND') {
    const missing = errors.make(Error, 'ENOENT', `spawn ${file} ENOENT`);
    return tag(missing, { errno: -2, syscall: `spawn ${file}`, path: file, spawnargs: args });
  }
  return error;
}

const errnoOf = (error) => (error.code === 'ENOENT' ? -2 : error.code === 'ERR_ACCESS_DENIED' ? -13 : -1);

function maxBufferError(name) {
  return errors.make(RangeError, 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER', `${name} maxBuffer length exceeded`);
}

function splitArgs(args, options) {
  if (args === undefined || args === null) return [[], options ?? {}];
  if (!Array.isArray(args)) return [[], args];
  return [args, options ?? {}];
}

function stdioModes(stdio) {
  const list = typeof stdio === 'string' ? [stdio, stdio, stdio] : Array.isArray(stdio) ? stdio : [];
  if (list.length > 3) throw unsupported('stdio entries beyond stdin, stdout and stderr');
  return [0, 1, 2].map((i) => {
    const wanted = list[i] ?? 'pipe';
    const mode = MODES[wanted];
    if (mode === undefined) throw unsupported(`stdio ${typeof wanted === 'string' ? JSON.stringify(wanted) : typeof wanted}`);
    return mode;
  });
}

function validate(options) {
  if (options === null || typeof options !== 'object') throw errors.invalidArgType('options', 'Object', options);
  if (options.detached) throw unsupported('detached');
  if (options.uid !== undefined || options.gid !== undefined) throw unsupported('uid and gid');
  if (options.cwd !== undefined && typeof options.cwd !== 'string') throw errors.invalidArgType('options.cwd', 'string', options.cwd);
  const { maxBuffer = DEFAULT_MAX_BUFFER, timeout = 0 } = options;
  if (typeof maxBuffer !== 'number' || !(maxBuffer >= 0) || maxBuffer > MAX_BUFFER_LIMIT) {
    throw errors.make(RangeError, 'ERR_OUT_OF_RANGE', `The value of "options.maxBuffer" is out of range. It must be a number between 0 and ${MAX_BUFFER_LIMIT} (64 MiB). Received ${String(maxBuffer)}`);
  }
  if (!Number.isInteger(timeout) || timeout < 0) {
    throw errors.make(RangeError, 'ERR_OUT_OF_RANGE', `The value of "options.timeout" is out of range. It must be an integer >= 0. Received ${String(timeout)}`);
  }
  return { maxBuffer: Math.floor(maxBuffer), timeout };
}

/** Everything a spawn needs, validated; the shell, if asked for, is just another program. */
function prepare(file, args, options) {
  if (typeof file !== 'string' || file === '') throw errors.invalidArgType('file', 'string', file);
  if (args.some((arg) => typeof arg !== 'string')) throw errors.invalidArgType('args', 'Array of string', args);
  const limits = validate(options);
  const shell = options.shell === true ? '/bin/sh' : options.shell;
  if (shell !== undefined && shell !== false && typeof shell !== 'string') throw errors.invalidArgType('options.shell', ['boolean', 'string'], shell);
  const command = [file, ...args].join(' ');
  const [program, argv] = shell ? [shell, ['-c', command]] : [file, [...args]];
  const source = options.env ?? process.env;
  if (source === null || typeof source !== 'object') throw errors.invalidArgType('options.env', 'Object', source);
  const env = Object.keys(source).filter((name) => source[name] !== undefined).map((name) => [name, String(source[name])]);
  const modes = stdioModes(options.stdio);
  return { program, argv, env, modes, cwd: options.cwd ?? null, command, ...limits };
}

function wire(spec) {
  const [stdin, stdout, stderr] = spec.modes;
  return { program: spec.program, args: spec.argv, cwd: spec.cwd, env: spec.env, stdin, stdout, stderr };
}

// ---- async ----------------------------------------------------------------------------------------

function readable(web) {
  const reader = web.getReader();
  return new Readable({
    highWaterMark: HIGH_WATER_MARK,
    read() {
      reader.read().then(({ value, done }) => {
        this.push(done ? null : Buffer.from(value.buffer, value.byteOffset, value.byteLength));
      }, (error) => this.destroy(error));
    },
    destroy(error, callback) {
      reader.cancel().then(() => callback(error), () => callback(error));
    },
  });
}

function writable(web) {
  const writer = web.getWriter();
  return new Writable({
    highWaterMark: HIGH_WATER_MARK,
    write(chunk, encoding, callback) {
      writer.write(chunk).then(() => callback(), callback);
    },
    final(callback) {
      writer.close().then(() => callback(), () => callback());
    },
    destroy(error, callback) {
      writer.close().then(() => callback(error), () => callback(error));
    },
  });
}

class ChildProcess extends EventEmitter {
  constructor() {
    super();
    this.pid = undefined;
    this.stdin = null;
    this.stdout = null;
    this.stderr = null;
    this.stdio = [null, null, null];
    this.exitCode = null;
    this.signalCode = null;
    this.killed = false;
    this.connected = false;
    this.spawnfile = '';
    this.spawnargs = [];
    Object.defineProperty(this, '_handle', { value: null, writable: true });
  }

  kill(signal) {
    if (signal !== undefined && signal !== 'SIGKILL' && signal !== 9) {
      throw unsupported(`child.kill(${String(signal)}): only SIGKILL`);
    }
    if (this._handle === null || this.exitCode !== null || this.signalCode !== null) return false;
    this._handle.kill();
    this.killed = true;
    return true;
  }

  ref() {}

  unref() {}
}

function finish(child, spec, handle, options) {
  let open = 1 + (child.stdout !== null) + (child.stderr !== null);
  const closed = () => {
    if (--open === 0) child.emit('close', child.exitCode, child.signalCode);
  };
  for (const stream of [child.stdout, child.stderr]) stream?.once('close', closed);
  let timer;
  if (spec.timeout > 0) timer = setTimeout(() => child.kill(), spec.timeout);
  if (options.signal) {
    const abort = () => {
      child.kill();
      child.emit('error', tag(errors.make(Error, 'ABORT_ERR', 'The operation was aborted'), { name: 'AbortError', cause: options.signal.reason }));
    };
    if (options.signal.aborted) process.nextTick(abort);
    else options.signal.addEventListener('abort', abort, { once: true });
  }
  handle.status.catch(() => ({ code: null, signal: null })).then((status) => {
    clearTimeout(timer);
    child.exitCode = status.code;
    child.signalCode = status.signal;
    child.emit('exit', status.code, status.signal);
    // Like Node: output nobody is reading must not keep 'close' from firing.
    for (const stream of [child.stdout, child.stderr]) if (stream && stream.listenerCount('data') === 0) stream.resume();
    closed();
  });
}

function start(child, spec, options) {
  child.spawnfile = spec.program;
  child.spawnargs = [spec.program, ...spec.argv];
  let handle;
  try {
    handle = new (velyr().Command)(spec.program, {
      args: spec.argv, cwd: spec.cwd ?? undefined, env: Object.fromEntries(spec.env),
      stdin: spec.modes[0], stdout: spec.modes[1], stderr: spec.modes[2],
    }).spawn();
  } catch (error) {
    const failure = translate(error, spec.program, spec.argv);
    if (failure === error && error?.code?.startsWith?.('VELYR_') !== true) throw error;
    process.nextTick(() => {
      child.exitCode = errnoOf(failure);
      child.emit('error', failure);
      child.emit('close', child.exitCode, null);
    });
    return child;
  }
  Object.defineProperty(child, '_handle', { value: handle, writable: true });
  child.pid = handle.pid;
  if (handle.stdin) child.stdin = writable(handle.stdin);
  if (handle.stdout) child.stdout = readable(handle.stdout);
  if (handle.stderr) child.stderr = readable(handle.stderr);
  child.stdio = [child.stdin, child.stdout, child.stderr];
  finish(child, spec, handle, options);
  process.nextTick(() => child.emit('spawn'));
  return child;
}

function spawn(file, args, options) {
  [args, options] = splitArgs(args, options);
  return start(new ChildProcess(), prepare(file, args, options), options);
}

// A forked child would be `velyr run <module> ...args`, and the runtime cannot hand it this
// process' permissions: it would parse flags the script chose (`-A`) and run with everything. So
// fork() is unsupported until permissions can be inherited over a channel the script does not
// control. io-process' gate also refuses the runtime's own executable, so spawn() cannot be used to
// the same end.
function fork() {
  throw unsupported('fork(): a forked runtime cannot inherit this process\' permissions, and child processes of the runtime itself are refused');
}

function run(file, args, options, callback) {
  if (typeof args === 'function') [args, options, callback] = [undefined, undefined, args];
  else if (typeof options === 'function') [options, callback] = [undefined, options];
  [args, options] = splitArgs(args, options);
  if (callback !== undefined && typeof callback !== 'function') throw errors.invalidArgType('callback', 'Function', callback);
  const { encoding = 'utf8', killSignal: _ignored, ...rest } = options;
  const spec = prepare(file, args, { ...rest, stdio: undefined });
  const command = spec.command;
  const child = new ChildProcess();
  const slots = { stdout: { chunks: [], size: 0 }, stderr: { chunks: [], size: 0 } };
  let failure = null;
  let reported = false;
  const report = (error) => {
    if (reported) return;
    reported = true;
    const text = (slot) => {
      const buffer = Buffer.concat(slot.chunks);
      return encoding !== 'buffer' && Buffer.isEncoding(encoding) ? buffer.toString(encoding) : buffer;
    };
    const [stdout, stderr] = [text(slots.stdout), text(slots.stderr)];
    if (error) tag(error, { stdout, stderr });
    if (callback) callback(error, stdout, stderr);
  };
  child.once('error', (error) => {
    failure = error;
    report(error);
  });
  child.once('close', (code, signal) => {
    let error = failure;
    if (!error && (code !== 0 || signal)) {
      const stderr = Buffer.concat(slots.stderr.chunks).toString();
      error = tag(new Error(`Command failed: ${command}\n${stderr}`), { code, killed: child.killed, signal, cmd: command });
    }
    report(error);
  });
  start(child, spec, {});
  for (const name of ['stdout', 'stderr']) {
    const slot = slots[name];
    child[name]?.on('data', (chunk) => {
      const room = spec.maxBuffer - slot.size;
      slot.size += chunk.length;
      if (chunk.length <= room) return slot.chunks.push(chunk);
      slot.chunks.push(chunk.subarray(0, room));
      slot.size = spec.maxBuffer;
      failure ??= tag(maxBufferError(name), { cmd: command });
      child.kill();
    });
  }
  const input = options.input;
  if (input !== undefined && child.stdin) child.stdin.end(input);
  return child;
}

function promisified(original) {
  return (...args) => {
    let child;
    const promise = new Promise((resolve, reject) => {
      child = original(...args, (error, stdout, stderr) => (error ? reject(error) : resolve({ stdout, stderr })));
    });
    return Object.assign(promise, { child });
  };
}

const execFile = run;

function exec(command, options, callback) {
  if (typeof command !== 'string') throw errors.invalidArgType('command', 'string', command);
  if (typeof options === 'function') [options, callback] = [undefined, options];
  return run(command, [], { ...options, shell: options?.shell || true }, callback);
}

Object.defineProperty(execFile, kCustomPromisify, { value: promisified(execFile) });
Object.defineProperty(exec, kCustomPromisify, { value: promisified(exec) });

// ---- sync -----------------------------------------------------------------------------------------

function toBytes(input) {
  if (input === undefined || input === null) return new Uint8Array(0);
  if (typeof input === 'string') return Buffer.from(input);
  if (ArrayBuffer.isView(input)) return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  throw errors.invalidArgType('options.input', ['string', 'Buffer', 'TypedArray', 'DataView'], input);
}

function spawnSync(file, args, options) {
  [args, options] = splitArgs(args, options);
  const spec = prepare(file, args, options);
  const input = toBytes(options.input);
  const result = { pid: 0, output: null, stdout: null, stderr: null, status: null, signal: null };
  let raw;
  try {
    raw = ops.childRunSync(JSON.stringify(wire(spec)), input, `${spec.timeout},${spec.maxBuffer}`);
  } catch (error) {
    const failure = translate(error, spec.program, spec.argv);
    if (failure === error && error?.code?.startsWith?.('VELYR_') !== true) throw error;
    result.error = failure;
    return result;
  }
  const view = new DataView(raw);
  const headerLength = view.getUint32(0);
  const header = JSON.parse(Buffer.from(raw, 4, headerLength).toString());
  const outStart = 4 + headerLength;
  const take = (buffer, mode) => {
    if (mode !== 'piped') return null;
    return options.encoding && options.encoding !== 'buffer' ? buffer.toString(options.encoding) : buffer;
  };
  const stdout = Buffer.from(raw, outStart, header.stdoutLen);
  const stderr = Buffer.from(raw, outStart + header.stdoutLen);
  result.stdout = take(stdout, spec.modes[1]);
  result.stderr = take(stderr, spec.modes[2]);
  result.output = [null, result.stdout, result.stderr];
  result.status = header.code;
  result.signal = header.signal === null ? null : (SIGNALS[header.signal] ?? String(header.signal));
  if (header.overflowed) result.error = maxBufferError(spec.modes[1] === 'piped' && stdout.length >= spec.maxBuffer ? 'stdout' : 'stderr');
  else if (header.timedOut) result.error = tag(errors.make(Error, 'ETIMEDOUT', `spawnSync ${spec.program} ETIMEDOUT`), { errno: -110, syscall: `spawnSync ${spec.program}` });
  return result;
}

function checked(result, options, command) {
  if (result.error) throw tag(result.error, { status: result.status, signal: result.signal, output: result.output, stdout: result.stdout, stderr: result.stderr, cmd: command });
  // Like Node, a child's stderr reaches ours unless stdio was configured explicitly.
  if (!options.stdio && result.stderr?.length) process.stderr.write(result.stderr);
  if (result.status !== 0) {
    const stderr = result.stderr === null ? '' : `\n${result.stderr.toString()}`;
    throw tag(new Error(`Command failed: ${command}${stderr}`), { status: result.status, signal: result.signal, output: result.output, pid: result.pid, stdout: result.stdout, stderr: result.stderr, cmd: command });
  }
  return result.stdout;
}

function execFileSync(file, args, options) {
  [args, options] = splitArgs(args, options);
  return checked(spawnSync(file, args, options), options, [file, ...args].join(' '));
}

function execSync(command, options = {}) {
  if (typeof command !== 'string') throw errors.invalidArgType('command', 'string', command);
  return checked(spawnSync(command, [], { ...options, shell: options.shell || true }), options, command);
}

module.exports = { ChildProcess, exec, execFile, execFileSync, execSync, fork, spawn, spawnSync };

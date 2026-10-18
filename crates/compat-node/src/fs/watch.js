'use strict';
// fs.watch, fs.watchFile/unwatchFile and fs.promises.watch on top of Velyr.watchFs (io-fs). One
// debounced event source (50 ms) feeds all three. Written for this runtime.
//
// Known limits: `persistent`, `interval` and `bigint` are accepted and ignored; ref()/unref() do
// nothing, so a live watcher always keeps the process alive; `watchFile` reacts to change events
// of the parent directory instead of polling, and a missing parent directory is an ENOENT.
const { EventEmitter } = require('events');
const { nextTick, platform } = require('process');
// Event paths come back from the OS in native form, so they are resolved and compared the same way.
const path = platform === 'win32' ? require('path').win32 : require('path');
const errors = require('internal/errors');
const { Buffer } = require('buffer');
const { toPath, getOptions } = require('internal/fs/util');
const { sync } = require('internal/fs/ops');
const { Stats } = require('internal/fs/stats');

function translate(error, target) {
  if (error.code === 'VELYR_PERMISSION_DENIED') {
    const denied = errors.make(Error, 'ERR_ACCESS_DENIED', error.message);
    Object.defineProperty(denied, 'cause', { value: error, writable: true, configurable: true });
    return denied;
  }
  if (error.code === 'VELYR_NOT_FOUND') {
    const missing = errors.make(Error, 'ENOENT', `ENOENT: no such file or directory, watch '${target}'`);
    Object.assign(missing, { errno: -2, syscall: 'watch', path: target });
    return missing;
  }
  return error;
}

const abortError = () => {
  const error = errors.make(Error, 'ABORT_ERR', 'The operation was aborted');
  error.name = 'AbortError';
  return error;
};

/**
 * Opens the event source for `target`: `{ events, name(path) }`, where `name` gives the
 * `filename` Node reports (relative to a watched directory, the base name of a watched file).
 */
function open(target, recursive, signal) {
  let events;
  try {
    events = globalThis.Velyr.watchFs([target], { recursive, signal });
  } catch (error) {
    throw translate(error, target);
  }
  try {
    const base = sync.realpathSync(target);
    const isFile = sync.statSync(base).isFile();
    return {
      events,
      name: (changed) => (isFile ? path.basename(base) : path.relative(base, changed) || path.basename(base)),
    };
  } catch (error) {
    events.close();
    throw error;
  }
}

const eventType = (kind) => (kind === 'modify' || kind === 'other' ? 'change' : 'rename');

class FSWatcher extends EventEmitter {
  constructor() {
    super();
    this._source = null;
    this._closed = false;
  }

  close() {
    if (this._closed) return;
    this._closed = true;
    this._source?.close();
    nextTick(() => this.emit('close'));
  }

  ref() { return this; }

  unref() { return this; }
}

function watch(filename, options, listener) {
  if (typeof options === 'function') [options, listener] = [undefined, options];
  const { recursive, encoding, signal } = getOptions(options);
  if (listener !== undefined) errors.validateFunction(listener, 'listener');
  const { events, name } = open(toPath(filename), recursive === true, signal);
  const watcher = new FSWatcher();
  watcher._source = events;
  if (listener) watcher.on('change', listener);
  (async () => {
    try {
      for await (const event of events) {
        for (const changed of event.paths) {
          const file = name(changed);
          watcher.emit('change', eventType(event.kind), encoding === 'buffer' ? Buffer.from(file) : file);
        }
      }
    } catch (error) {
      if (!watcher._closed) watcher.emit('error', error);
    }
    watcher.close();
  })();
  return watcher;
}

async function* watchPromise(filename, options) {
  const { recursive, encoding, signal } = getOptions(options);
  if (signal?.aborted) throw abortError();
  const { events, name } = open(toPath(filename), recursive === true, signal);
  try {
    for await (const event of events) {
      for (const changed of event.paths) {
        const file = name(changed);
        yield { eventType: eventType(event.kind), filename: encoding === 'buffer' ? Buffer.from(file) : file };
      }
    }
  } finally {
    events.close();
  }
  if (signal?.aborted) throw abortError();
}

const statWatchers = new Map();

const zeroStats = () => new Stats(new Array(14).fill('0'));
const statOf = (file) => sync.statSync(file, { throwIfNoEntry: false }) ?? zeroStats();
const differs = (a, b) => a.mtimeMs !== b.mtimeMs || a.size !== b.size || a.ino !== b.ino;

class StatWatcher extends EventEmitter {
  constructor(file) {
    super();
    this._file = path.resolve(file);
    this._source = null;
  }

  /** Watches the parent directory and reports the file when its stats change. */
  start() {
    const { events, name } = open(path.dirname(this._file), false);
    this._source = events;
    const target = path.basename(this._file);
    let previous = statOf(this._file);
    (async () => {
      try {
        for await (const event of events) {
          if (!event.paths.some((changed) => name(changed) === target)) continue;
          const current = statOf(this._file);
          if (differs(previous, current)) {
            const before = previous;
            previous = current;
            this.emit('change', current, before);
          }
        }
      } catch (error) {
        this.emit('error', error);
      }
    })();
  }

  stop() { this._source?.close(); }

  ref() { return this; }

  unref() { return this; }
}

function watchFile(filename, options, listener) {
  if (typeof options === 'function') listener = options;
  errors.validateFunction(listener, 'listener');
  const file = path.resolve(toPath(filename));
  let watcher = statWatchers.get(file);
  if (!watcher) {
    watcher = new StatWatcher(file);
    watcher.start();
    statWatchers.set(file, watcher);
  }
  watcher.on('change', listener);
  return watcher;
}

function unwatchFile(filename, listener) {
  const file = path.resolve(toPath(filename));
  const watcher = statWatchers.get(file);
  if (!watcher) return;
  if (typeof listener === 'function') watcher.removeListener('change', listener);
  else watcher.removeAllListeners('change');
  if (watcher.listenerCount('change') === 0) {
    watcher.stop();
    statWatchers.delete(file);
  }
}

module.exports = { watch, watchPromise, watchFile, unwatchFile, FSWatcher, StatWatcher };

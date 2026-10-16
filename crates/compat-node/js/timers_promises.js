'use strict';
// node:timers/promises. Written for this runtime; no Node source is copied.
const errors = require('internal/errors');
const timers = require('timers');

function abortError(signal) {
  const error = new Error('The operation was aborted', signal && signal.reason !== undefined ? { cause: signal.reason } : undefined);
  error.name = 'AbortError';
  error.code = 'ABORT_ERR';
  return error;
}

function checkOptions(options) {
  if (options === undefined || options === null) return {};
  errors.validateObject(options, 'options');
  return options;
}

/** Runs `schedule(resolve)` and rejects with an AbortError if `signal` fires first. */
function guarded(signal, schedule, cancel) {
  return new Promise((resolve, reject) => {
    if (signal && signal.aborted) { reject(abortError(signal)); return; }
    const handle = schedule(() => {
      if (signal) signal.removeEventListener('abort', onAbort);
      resolve();
    });
    function onAbort() {
      cancel(handle);
      reject(abortError(signal));
    }
    if (signal) signal.addEventListener('abort', onAbort, { once: true });
  });
}

/** Applies the `ref: false` option; ref defaults to true. */
const withRef = (handle, ref) => (ref === false ? handle.unref() : handle);

function setTimeout(delay, value, options) {
  const { signal, ref } = checkOptions(options);
  return guarded(signal, (done) => withRef(timers.setTimeout(done, delay), ref), timers.clearTimeout).then(() => value);
}

function setImmediate(value, options) {
  const { signal, ref } = checkOptions(options);
  return guarded(signal, (done) => withRef(timers.setImmediate(done), ref), timers.clearImmediate).then(() => value);
}

async function* setInterval(delay, value, options) {
  const { signal, ref } = checkOptions(options);
  if (signal && signal.aborted) throw abortError(signal);
  let ready = 0;
  let wake;
  const handle = withRef(timers.setInterval(() => { ready++; if (wake) { wake(); wake = undefined; } }, delay), ref);
  const onAbort = () => { if (wake) { wake(); wake = undefined; } };
  if (signal) signal.addEventListener('abort', onAbort, { once: true });
  try {
    for (;;) {
      if (signal && signal.aborted) throw abortError(signal);
      if (ready === 0) await new Promise((resolve) => { wake = resolve; });
      if (signal && signal.aborted) throw abortError(signal);
      for (; ready > 0; ready--) yield value;
    }
  } finally {
    timers.clearInterval(handle);
    if (signal) signal.removeEventListener('abort', onAbort);
  }
}

const scheduler = {
  wait: (delay, options) => setTimeout(delay, undefined, options),
  yield: () => setImmediate(),
};

module.exports = { setTimeout, setImmediate, setInterval, scheduler };

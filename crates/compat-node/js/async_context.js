'use strict';
// internal/async_context: the "frame" that follows asynchronous work, a Map of AsyncLocalStorage
// to store that is never mutated once set (every `run` makes a new one). Where the engine has a
// continuation context (V8) the frame lives there, so promise reactions and `await` restore it by
// themselves; this module only has to carry it across what the engine cannot see: timers, ticks
// and immediates. Elsewhere the frame is a plain variable, which covers the synchronous path and
// those same carriers but not `await`. Written for this runtime.
const ops = require('internal/ops');

let fallback;

function engineHasContext() {
  try {
    const probe = {};
    ops.ctxSet(probe);
    const kept = ops.ctxGet() === probe;
    ops.ctxSet(undefined);
    return kept;
  } catch {
    return false;
  }
}

const supported = engineHasContext();
const current = supported ? () => ops.ctxGet() : () => fallback;
const enter = supported ? (frame) => { ops.ctxSet(frame); } : (frame) => { fallback = frame; };

// Runs `fn` inside `frame`, then puts back the frame that was there.
function run(frame, fn, thisArg, args) {
  const previous = current();
  enter(frame);
  try {
    return Reflect.apply(fn, thisArg, args);
  } finally {
    enter(previous);
  }
}

// What web-core's timers use (they are not Node's, so they find it by symbol): the frame at
// `setTimeout` time comes back around the callback.
Object.defineProperty(globalThis, Symbol.for('velyr.asyncContext'), {
  value: Object.freeze({ current, run }),
});

module.exports = { supported, current, enter, run };

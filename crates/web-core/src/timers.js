// Bootstrap: timers and queueMicrotask on top of the private op_timer_* binding.
// Evaluated once per isolate; its completion value is the dispatcher the event loop calls
// with a timer id when that timer fires.
(function (global) {
  "use strict";

  // Capture the raw ops, then remove them so user code never sees them (ADR-010).
  const setOp = global.op_timer_set;
  const clearOp = global.op_timer_clear;
  const refOp = global.op_timer_ref;
  delete global.op_timer_set;
  delete global.op_timer_clear;
  delete global.op_timer_ref;
  const checkpointOp = global.op_microtask_checkpoint;
  delete global.op_microtask_checkpoint;
  const microtaskOp = global.op_queue_microtask;
  delete global.op_queue_microtask;

  const apply = Reflect.apply;
  const ASYNC_CONTEXT = Symbol.for("velyr.asyncContext");
  const TICK = Symbol.for("velyr.tick");
  const timers = new Map();
  // Nesting level of the batched callback being run (0 outside a batch), and batch counters.
  let nesting = 0;
  let batches = 0;
  let fired = 0;

  function define(name, value) {
    Object.defineProperty(global, name, {
      value,
      writable: true,
      configurable: true,
      enumerable: true,
    });
  }

  function requireCallback(callback) {
    if (typeof callback !== "function") {
      throw new TypeError("The callback provided must be a function");
    }
  }

  // A throw in a microtask is an uncaught exception, not a rejection and not a skipped microtask.
  function report(e) {
    if (typeof global.reportError === "function") global.reportError(e);
    else setTimeout(() => { throw e; }, 0);
  }

  // The op returns false on engines without a microtask queue (engine-native): use a promise job.
  function queueRun(run) {
    if (!microtaskOp(run)) Promise.resolve().then(run);
  }

  function schedule(repeat, callback, delay, args) {
    // shortcut: string handlers (eval) are rejected; HTML allows them, nobody needs them here.
    requireCallback(callback);
    // ToInt32 like the HTML `long` conversion; negative and NaN delays mean 0.
    // Inside a batch the loop cannot know the running level, so the new timer's level goes along.
    const id = setOp(Math.max(0, delay | 0), repeat, nesting === 0 ? 0 : nesting + 1);
    // The async context (AsyncLocalStorage's frame) at schedule time; see dispatch.
    const context = global[ASYNC_CONTEXT];
    timers.set(id, { callback, args, repeat, frame: context && context.current() });
    return id;
  }

  function cancel(id) {
    const key = id | 0;
    if (!timers.delete(key)) return;
    clearOp(key);
  }

  define("setTimeout", function setTimeout(callback, delay, ...args) {
    return schedule(false, callback, delay, args);
  });
  define("setInterval", function setInterval(callback, delay, ...args) {
    return schedule(true, callback, delay, args);
  });
  define("clearTimeout", function clearTimeout(id) {
    cancel(id);
  });
  define("clearInterval", function clearInterval(id) {
    cancel(id);
  });
  define("queueMicrotask", function queueMicrotask(callback) {
    requireCallback(callback);
    queueRun(() => {
      try {
        callback();
      } catch (e) {
        report(e);
      }
    });
  });

  // Hook for the Node layer's Timeout.ref()/unref(): an unref'd timer fires but does not keep
  // the loop alive. Under a symbol key so it stays out of the way of user code; unknown ids are no-ops.
  Object.defineProperty(global, Symbol.for("velyr.timers"), {
    value: Object.freeze({
      ref(id, refd) {
        const key = id | 0;
        if (timers.has(key)) refOp(key, !!refd);
      },
      stats() {
        return { batches, fired };
      },
    }),
  });

  function run(id) {
    const timer = timers.get(id);
    if (timer === undefined) return;
    if (!timer.repeat) timers.delete(id);
    // Installed by the Node layer when it is loaded; it puts the frame back around the callback.
    const context = global[ASYNC_CONTEXT];
    if (context) context.run(timer.frame, timer.callback, global, timer.args);
    else apply(timer.callback, global, timer.args);
  }

  // What the loop does after each callback; between batched callbacks it is done here. The tick
  // queue is published by the Node layer when it loads (absent: there are no ticks).
  function drain() {
    const tick = global[TICK];
    if (typeof tick === "function") tick();
    checkpointOp();
  }

  // dispatch(id0, level0, id1, level1, ...): every due host timer in one call. A throw does not
  // cancel the rest; the first is rethrown at the end and the others are reported.
  function runBatch(list) {
    batches++;
    let failed = false;
    let failure;
    const fail = (e) => {
      if (failed) report(e);
      else {
        failed = true;
        failure = e;
      }
    };
    for (let i = 0; i < list.length; i += 2) {
      const id = list[i];
      // Cleared by an earlier callback of the batch.
      if (!timers.has(id)) continue;
      nesting = list[i + 1];
      fired++;
      try {
        run(id);
      } catch (e) {
        fail(e);
      }
      try {
        drain();
      } catch (e) {
        fail(e);
      }
    }
    nesting = 0;
    if (failed) throw failure;
  }

  return function dispatch(id) {
    if (arguments.length > 1) runBatch(arguments);
    else run(id);
  };
})(globalThis);

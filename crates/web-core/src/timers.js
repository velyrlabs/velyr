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

  const apply = Reflect.apply;
  const ASYNC_CONTEXT = Symbol.for("velyr.asyncContext");
  const timers = new Map();

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

  function schedule(repeat, callback, delay, args) {
    // shortcut: string handlers (eval) are rejected; HTML allows them, nobody needs them here.
    requireCallback(callback);
    // ToInt32 like the HTML `long` conversion; negative and NaN delays mean 0.
    const id = setOp(Math.max(0, delay | 0), repeat);
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
    Promise.resolve().then(() => {
      callback();
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
    }),
  });

  return function dispatch(id) {
    const timer = timers.get(id);
    if (timer === undefined) return;
    if (!timer.repeat) timers.delete(id);
    // Installed by the Node layer when it is loaded; it puts the frame back around the callback.
    const context = global[ASYNC_CONTEXT];
    if (context) context.run(timer.frame, timer.callback, global, timer.args);
    else apply(timer.callback, global, timer.args);
  };
})(globalThis);

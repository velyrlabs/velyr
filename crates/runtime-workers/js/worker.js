// Bootstrap: Worker and Velyr.unrefWorker (ADR-014). Called with the toolkit messaging.js returned.
(function (toolkit) {
  "use strict";

  const { op, MessageEvent, ErrorEvent, send, startPump, defineHandler } = toolkit;
  const { Event } = globalThis;
  const states = new WeakMap();

  function workerState(worker) {
    const state = states.get(worker);
    if (state === undefined) throw new TypeError("Illegal invocation");
    return state;
  }

  function checkOptions(options) {
    if (options === null || typeof options !== "object") throw new TypeError("Worker options must be an object");
    const type = options.type === undefined ? "module" : String(options.type);
    if (type !== "module") {
      throw new TypeError("Worker type '" + type + "' is not supported, only 'module' workers are");
    }
    const permissions = options.velyr?.permissions;
    if (permissions !== undefined && (permissions === null || typeof permissions !== "object")) {
      throw new TypeError("velyr.permissions must be an object");
    }
    const heapMb = options.velyr?.heapLimitMb;
    if (heapMb !== undefined && !(Number.isFinite(heapMb) && heapMb >= 1)) {
      throw new TypeError("velyr.heapLimitMb must be a number of at least 1");
    }
    return {
      permissions: permissions === undefined ? undefined : JSON.stringify(permissions),
      heapBytes: heapMb === undefined ? undefined : Math.floor(heapMb) * 1024 * 1024,
    };
  }

  // The module that ran `new Worker`, to resolve relative URLs against.
  // shortcut: first `file://` frame of the V8 stack; undefined (cwd fallback) when absent.
  function callerUrl() {
    const frame = /(file:\/\/[^\s()]*?):\d+:\d+/.exec(new Error().stack ?? "");
    return frame === null ? undefined : frame[1];
  }

  class Worker extends EventTarget {
    constructor(url, options = {}) {
      if (arguments.length === 0) throw new TypeError("Worker requires 1 argument");
      const { permissions, heapBytes } = checkOptions(options);
      super();
      const href = url instanceof URL ? url.href : String(url);
      const [workerId, id] = op.worker_spawn(href, String(options.name ?? ""), permissions, callerUrl(), heapBytes);
      const state = { workerId, id, refd: true, pumping: false, terminated: false };
      states.set(this, state);
      startPump(this, state, {
        message: (item) => {
          if (item.kind === 2) {
            // Internal, not a web event: the worker's isolate is gone. Always after any `error`.
            const ended = new Event("ended");
            Object.defineProperty(ended, "code", { value: item.value, enumerable: true });
            this.dispatchEvent(ended);
          } else if (item.kind === 1) {
            const { message, filename, lineno, colno, quota } = item.value;
            const error = new Error(message);
            if (quota !== undefined) Object.assign(error, { code: "VELYR_QUOTA_EXCEEDED", quota });
            this.dispatchEvent(new ErrorEvent("error", { message, filename, lineno, colno, error, cancelable: true }));
          } else {
            this.dispatchEvent(new MessageEvent("message", { data: item.value, ports: item.ports }));
          }
        },
        messageerror: () => this.dispatchEvent(new MessageEvent("messageerror")),
        // The worker's thread is gone: free its slot.
        closed: () => op.worker_terminate(workerId),
      });
    }

    postMessage(message, transfer) {
      if (arguments.length === 0) throw new TypeError("postMessage requires 1 argument");
      const state = workerState(this);
      if (state.id !== null) send(state.id, message, transfer, null);
    }

    terminate() {
      const state = workerState(this);
      if (state.terminated) return;
      state.terminated = true;
      const { id } = state;
      state.id = null;
      if (id !== null) op.port_close(id);
      op.worker_terminate(state.workerId);
    }

    get [Symbol.toStringTag]() { return "Worker"; }
  }
  for (const type of ["message", "messageerror", "error"]) defineHandler(Worker.prototype, type);

  // Lets the parent exit while `worker` is still running; its messages keep arriving while
  // something else keeps the loop alive. Not a method, so Worker.prototype stays standard.
  function unrefWorker(worker) {
    const state = workerState(worker);
    if (!state.refd) return;
    state.refd = false;
    if (state.id !== null) op.port_interrupt(state.id);
  }

  Object.defineProperty(globalThis, "Worker", {
    value: Worker, writable: true, configurable: true, enumerable: false,
  });
  Object.defineProperty(globalThis.Velyr, "unrefWorker", {
    value: unrefWorker, writable: false, configurable: true, enumerable: true,
  });
})

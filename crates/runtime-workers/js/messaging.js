// Bootstrap: MessageEvent, ErrorEvent, MessagePort, MessageChannel and BroadcastChannel on top of
// the private op_port_* / op_bc_* bindings. Evaluates to the toolkit worker.js and worker_scope.js use.
(function (global) {
  "use strict";

  // Capture the raw ops, then remove them so user code never sees them (ADR-010).
  const op = {};
  for (const name of [
    "port_pair", "port_post", "port_recv", "port_take", "port_interrupt", "port_close",
    "bc_open", "worker_spawn", "worker_terminate", "worker_scope_port", "worker_close",
    "worker_report_error",
  ]) {
    op[name] = global["op_" + name];
    delete global["op_" + name];
  }
  // The raw worker-side messaging ops are superseded by the port ops.
  for (const name of ["op_worker_post_message", "op_worker_recv_message", "op_worker_take_message"]) {
    delete global[name];
  }

  const { Event, EventTarget, DOMException } = global;
  // Marks a transferred port inside a cloned message; the receiver swaps it for the real port.
  const MARK = "\u0000velyr:port";
  const PORT_TOKEN = Symbol("MessagePort");
  // MessagePort, BroadcastChannel: { id, refd, pumping, ... }; id is null once closed or transferred.
  const states = new WeakMap();

  function define(name, value) {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  }

  function stateOf(target, Class) {
    const state = states.get(target);
    if (state === undefined || !(target instanceof Class)) throw new TypeError("Illegal invocation");
    return state;
  }

  class MessageEvent extends Event {
    #data; #origin; #lastEventId; #source; #ports;

    constructor(type, init = {}) {
      super(type, init);
      const dict = init ?? {};
      this.#data = dict.data ?? null;
      this.#origin = String(dict.origin ?? "");
      this.#lastEventId = String(dict.lastEventId ?? "");
      this.#source = dict.source ?? null;
      this.#ports = Object.freeze([...(dict.ports ?? [])]);
    }

    get data() { return this.#data; }
    get origin() { return this.#origin; }
    get lastEventId() { return this.#lastEventId; }
    get source() { return this.#source; }
    get ports() { return this.#ports; }
    get [Symbol.toStringTag]() { return "MessageEvent"; }
  }

  class ErrorEvent extends Event {
    #message; #filename; #lineno; #colno; #error;

    constructor(type, init = {}) {
      super(type, init);
      const dict = init ?? {};
      this.#message = String(dict.message ?? "");
      this.#filename = String(dict.filename ?? "");
      this.#lineno = Number(dict.lineno ?? 0) >>> 0;
      this.#colno = Number(dict.colno ?? 0) >>> 0;
      this.#error = dict.error;
    }

    get message() { return this.#message; }
    get filename() { return this.#filename; }
    get lineno() { return this.#lineno; }
    get colno() { return this.#colno; }
    get error() { return this.#error; }
    get [Symbol.toStringTag]() { return "ErrorEvent"; }
  }

  function cloneError(message) {
    return new DOMException(message, "DataCloneError");
  }

  // Splits a transfer list (iterable, or `{ transfer }`) into ArrayBuffers and MessagePorts.
  function splitTransfer(transfer, self) {
    const buffers = [];
    const ports = [];
    let list = transfer;
    if (list !== undefined && list !== null && typeof list === "object" && typeof list[Symbol.iterator] !== "function") {
      list = list.transfer;
    }
    if (list === undefined || list === null) return { buffers, ports };
    if (typeof list[Symbol.iterator] !== "function") throw new TypeError("transfer must be iterable");
    for (const item of Array.from(list)) {
      if (item instanceof MessagePort) {
        if (item === self || ports.includes(item)) throw cloneError("Port is already in the transfer list");
        if (states.get(item).id === null) throw cloneError("Port is closed or was already transferred");
        ports.push(item);
      } else if (item instanceof ArrayBuffer) {
        if (buffers.includes(item)) throw cloneError("ArrayBuffer is already in the transfer list");
        buffers.push(item);
      } else {
        throw cloneError("Value not transferable");
      }
    }
    return { buffers, ports };
  }

  // Structured-clones `message` to the peer of endpoint `id`.
  function send(id, message, transfer, self) {
    const { buffers, ports } = splitTransfer(transfer, self);
    ports.forEach((port, index) => {
      Object.defineProperty(port, MARK, { value: index, enumerable: true, configurable: true });
    });
    try {
      op.port_post(id, message, buffers, ports.map((port) => states.get(port).id));
    } catch (error) {
      if (error?.name === "DataCloneError") throw cloneError(error.message);
      throw error;
    } finally {
      for (const port of ports) delete port[MARK];
    }
    for (const port of ports) states.get(port).id = null;
  }

  function newPort(id) {
    return new MessagePort(PORT_TOKEN, id);
  }

  // shortcut: ports are revived inside plain objects and arrays only, not Map/Set values;
  // upgrade by walking those collections too.
  function revive(value, ports) {
    const seen = new Set();
    const visit = (node) => {
      if (node === null || typeof node !== "object" || ArrayBuffer.isView(node) || node instanceof ArrayBuffer) {
        return node;
      }
      if (Object.hasOwn(node, MARK) && typeof node[MARK] === "number" && ports[node[MARK]] !== undefined) {
        return ports[node[MARK]];
      }
      if (seen.has(node)) return node;
      seen.add(node);
      for (const key of Object.keys(node)) {
        const next = visit(node[key]);
        if (next !== node[key]) node[key] = next;
      }
      return node;
    };
    return visit(value);
  }

  // Next packet of endpoint `id`: { kind: 0, value, ports } or { kind: 1, value: { message, filename, lineno, colno } }.
  function take(id) {
    const raw = op.port_take(id);
    if (raw === undefined) return undefined;
    const [kind, value, ids] = raw;
    if (kind === 1) return { kind, value };
    const ports = ids.map(newPort);
    return { kind, value: ports.length > 0 ? revive(value, ports) : value, ports };
  }

  // Delivers packets of `state.id` until the endpoint closes. A ref'd wait keeps the event loop
  // alive; `state.refd = false` plus `op.port_interrupt` switches to an unref'd one.
  function startPump(target, state, handlers) {
    if (state.pumping || state.id === null) return;
    state.pumping = true;
    (async () => {
      const id = state.id;
      for (;;) {
        let code;
        try {
          code = await op.port_recv(id, state.refd !== false);
        } catch {
          break;
        }
        if (state.id !== id) break;
        if (code === 0) {
          state.id = null;
          op.port_close(id);
          handlers.closed?.();
          break;
        }
        if (code === 2) continue;
        let item;
        try {
          item = take(id);
        } catch {
          handlers.messageerror();
          continue;
        }
        if (item !== undefined) handlers.message(item);
      }
    })();
  }

  // `on<type>` event handler attribute for `object`; `onSet` runs after a function is assigned.
  function defineHandler(object, type, onSet) {
    const handlers = new WeakMap();
    Object.defineProperty(object, "on" + type, {
      get() { return handlers.get(this)?.fn ?? null; },
      set(value) {
        let record = handlers.get(this);
        if (record === undefined) {
          record = { fn: null };
          handlers.set(this, record);
          this.addEventListener(type, (event) => {
            if (record.fn !== null) Reflect.apply(record.fn, this, [event]);
          });
        }
        record.fn = typeof value === "function" ? value : null;
        if (record.fn !== null) onSet?.(this);
      },
      configurable: true,
      enumerable: true,
    });
  }

  function messageHandlers(target) {
    return {
      message: (item) => target.dispatchEvent(new MessageEvent("message", { data: item.value, ports: item.ports })),
      messageerror: () => target.dispatchEvent(new MessageEvent("messageerror")),
      closed: () => target.dispatchEvent(new Event("close")),
    };
  }

  class MessagePort extends EventTarget {
    constructor(token, id) {
      if (token !== PORT_TOKEN) throw new TypeError("Illegal constructor");
      super();
      states.set(this, { id, refd: true, pumping: false });
    }

    postMessage(message, transfer) {
      if (arguments.length === 0) throw new TypeError("postMessage requires 1 argument");
      const state = stateOf(this, MessagePort);
      if (state.id !== null) send(state.id, message, transfer, this);
    }

    start() {
      startPump(this, stateOf(this, MessagePort), messageHandlers(this));
    }

    close() {
      const state = stateOf(this, MessagePort);
      if (state.id === null) return;
      const { id } = state;
      state.id = null;
      op.port_close(id);
    }

    get [Symbol.toStringTag]() { return "MessagePort"; }
  }
  defineHandler(MessagePort.prototype, "message", (port) => port.start());
  defineHandler(MessagePort.prototype, "messageerror");

  class MessageChannel {
    #port1; #port2;

    constructor() {
      const [first, second] = op.port_pair();
      this.#port1 = newPort(first);
      this.#port2 = newPort(second);
    }

    get port1() { return this.#port1; }
    get port2() { return this.#port2; }
    get [Symbol.toStringTag]() { return "MessageChannel"; }
  }

  class BroadcastChannel extends EventTarget {
    #name;

    constructor(name) {
      if (arguments.length === 0) throw new TypeError("BroadcastChannel requires 1 argument");
      super();
      this.#name = String(name);
      states.set(this, { id: op.bc_open(this.#name), refd: true, pumping: false });
    }

    get name() { return this.#name; }

    postMessage(message) {
      if (arguments.length === 0) throw new TypeError("postMessage requires 1 argument");
      const state = stateOf(this, BroadcastChannel);
      if (state.id === null) throw new DOMException("BroadcastChannel is closed.", "InvalidStateError");
      send(state.id, message, undefined, null);
    }

    close() {
      const state = stateOf(this, BroadcastChannel);
      if (state.id === null) return;
      const { id } = state;
      state.id = null;
      op.port_close(id);
    }

    // Delivery starts with the first message listener and keeps the loop alive until close().
    addEventListener(type, ...rest) {
      if (type === "message") startPump(this, stateOf(this, BroadcastChannel), messageHandlers(this));
      return super.addEventListener(type, ...rest);
    }

    get [Symbol.toStringTag]() { return "BroadcastChannel"; }
  }
  defineHandler(BroadcastChannel.prototype, "message");
  defineHandler(BroadcastChannel.prototype, "messageerror");

  define("MessageEvent", MessageEvent);
  define("ErrorEvent", ErrorEvent);
  define("MessagePort", MessagePort);
  define("MessageChannel", MessageChannel);
  define("BroadcastChannel", BroadcastChannel);

  return { op, MessageEvent, ErrorEvent, send, startPump, defineHandler, messageHandlers };
})(globalThis);

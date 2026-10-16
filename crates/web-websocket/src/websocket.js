// Bootstrap: WebSocket (WHATWG) and Velyr.upgradeWebSocket. Needs events, url, encoding, Blob,
// and the Velyr.serve hook installed by web-fetch.
(function (global) {
  "use strict";

  const { EventTarget, Event, DOMException, Blob } = global;
  const OP_NAMES = ["connect", "upgrade", "next", "send", "buffered", "close", "release", "accept_key"];
  const op = {};
  for (const name of OP_NAMES) {
    op[name] = global[`op_ws_${name}`];
    delete global[`op_ws_${name}`];
  }
  const blobHook = global[Symbol.for("velyr.internal.blob")];
  const serveHook = global[Symbol.for("velyr.internal.serve")];

  const CONNECTING = 0, OPEN = 1, CLOSING = 2, CLOSED = 3;
  const TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  const MAX_REASON_BYTES = 123;
  const encoder = new TextEncoder();

  class CloseEvent extends Event {
    #init;
    constructor(type, init = {}) {
      super(type, init);
      this.#init = { wasClean: !!init?.wasClean, code: Number(init?.code ?? 0) & 0xffff, reason: String(init?.reason ?? "") };
    }
    get wasClean() { return this.#init.wasClean; }
    get code() { return this.#init.code; }
    get reason() { return this.#init.reason; }
    get [Symbol.toStringTag]() { return "CloseEvent"; }
  }

  // Private fallback for runtimes without workers (which define the real MessageEvent).
  class FallbackMessageEvent extends Event {
    #data;
    constructor(type, init = {}) { super(type, init); this.#data = init?.data ?? null; }
    get data() { return this.#data; }
    get origin() { return ""; }
    get lastEventId() { return ""; }
    get source() { return null; }
    get ports() { return Object.freeze([]); }
    get [Symbol.toStringTag]() { return "MessageEvent"; }
  }

  const states = new WeakMap();
  const HANDLERS = ["open", "message", "error", "close"];

  function stateOf(target) {
    const state = states.get(target);
    if (state === undefined) throw new TypeError("Illegal invocation");
    return state;
  }

  function parseURL(input) {
    let url;
    try { url = new URL(String(input)); } catch { throw new DOMException(`Invalid URL: ${input}`, "SyntaxError"); }
    if (url.protocol === "http:") url.protocol = "ws:";
    else if (url.protocol === "https:") url.protocol = "wss:";
    if (url.protocol !== "ws:" && url.protocol !== "wss:") {
      throw new DOMException(`Unsupported WebSocket scheme: ${url.protocol}`, "SyntaxError");
    }
    if (url.hash !== "") throw new DOMException("The URL contains a fragment", "SyntaxError");
    return url.href;
  }

  function parseProtocols(input) {
    const list = input === undefined ? [] : typeof input === "string" ? [input] : Array.from(input, String);
    for (const protocol of list) {
      if (!TOKEN.test(protocol)) throw new DOMException(`Invalid subprotocol: ${protocol}`, "SyntaxError");
    }
    if (new Set(list).size !== list.length) throw new DOMException("Duplicate subprotocols", "SyntaxError");
    return list;
  }

  function makeMessage(state, data) {
    const Message = global.MessageEvent ?? FallbackMessageEvent;
    return new Message("message", { data });
  }

  async function pump(socket, state) {
    try {
      for (;;) {
        const item = await op.next(state.id);
        if (item === null) break;
        if (typeof item !== "string") {
          const data = state.binaryType === "blob" ? new Blob([item]) : item;
          if (state.readyState === OPEN) socket.dispatchEvent(makeMessage(state, data));
          continue;
        }
        const event = JSON.parse(item);
        if (event.t === "open") {
          if (state.readyState !== CONNECTING) continue;
          state.readyState = OPEN;
          state.protocol = event.protocol;
          socket.dispatchEvent(new Event("open"));
        } else if (event.t === "text") {
          if (state.readyState === OPEN) socket.dispatchEvent(makeMessage(state, event.data));
        } else if (event.t === "error") {
          socket.dispatchEvent(new Event("error"));
        } else if (event.t === "close") {
          finish(socket, state, event.code, event.reason, event.clean);
          break;
        }
      }
    } finally {
      if (state.readyState !== CLOSED) finish(socket, state, 1006, "", false);
    }
  }

  function finish(socket, state, code, reason, clean) {
    if (state.readyState === CLOSED) return;
    state.readyState = CLOSED;
    if (state.id !== null) { op.release(state.id); state.id = null; }
    socket.dispatchEvent(new CloseEvent("close", { code, reason, wasClean: clean }));
  }

  // Only the server side holds this, so only Velyr.upgradeWebSocket can build an accepted socket.
  const ACCEPTED = Object.freeze({});

  function newState(url) {
    return { id: null, readyState: CONNECTING, protocol: "", url, binaryType: "blob", handlers: {}, listeners: {} };
  }

  class WebSocket extends EventTarget {
    constructor(url, protocols) {
      if (arguments.length === 0) throw new TypeError("Failed to construct 'WebSocket': 1 argument required");
      super();
      if (url === ACCEPTED) {
        const accepted = newState("");
        accepted.id = protocols;
        states.set(this, accepted);
        pump(this, accepted);
        return;
      }
      const href = parseURL(url);
      const list = parseProtocols(protocols);
      const state = newState(href);
      states.set(this, state);
      let id;
      try {
        id = op.connect(href, JSON.stringify(list));
      } catch (error) {
        if (error?.code !== "VELYR_PERMISSION_DENIED") throw error;
        // Denied connections fail asynchronously like any network failure.
        queueMicrotask(() => {
          state.readyState = CLOSED;
          this.dispatchEvent(new Event("error"));
          this.dispatchEvent(new CloseEvent("close", { code: 1006, reason: "", wasClean: false }));
        });
        return;
      }
      state.id = id;
      pump(this, state);
    }

    get url() { return stateOf(this).url; }
    get readyState() { return stateOf(this).readyState; }
    get protocol() { return stateOf(this).protocol; }
    get extensions() { stateOf(this); return ""; }
    get bufferedAmount() {
      const state = stateOf(this);
      return state.id === null ? 0 : op.buffered(state.id);
    }
    get binaryType() { return stateOf(this).binaryType; }
    set binaryType(value) {
      const state = stateOf(this);
      value = String(value);
      if (value === "blob" || value === "arraybuffer") state.binaryType = value;
    }

    send(data) {
      const state = stateOf(this);
      if (state.readyState === CONNECTING) throw new DOMException("The WebSocket is still in CONNECTING state", "InvalidStateError");
      let payload;
      if (typeof data === "string") payload = data;
      else if (blobHook.isBlob(data)) payload = blobHook.readBytes(data);
      else if (data instanceof ArrayBuffer) payload = data;
      else if (ArrayBuffer.isView(data)) payload = data;
      else payload = String(data);
      if (state.readyState !== OPEN) return;
      op.send(state.id, payload);
    }

    close(code, reason) {
      const state = stateOf(this);
      if (code !== undefined) {
        code = Number(code) & 0xffff;
        if (code !== 1000 && (code < 3000 || code > 4999)) {
          throw new DOMException("The close code must be 1000 or in the range 3000-4999", "InvalidAccessError");
        }
      }
      reason = reason === undefined ? "" : String(reason);
      if (encoder.encode(reason).length > MAX_REASON_BYTES) {
        throw new DOMException("The close reason must not be greater than 123 bytes", "SyntaxError");
      }
      if (state.readyState === CLOSING || state.readyState === CLOSED) return;
      if (state.readyState === CONNECTING) {
        // Fail the connection: no handshake to complete.
        const id = state.id;
        state.readyState = CLOSING;
        state.id = null;
        if (id !== null) op.release(id);
        queueMicrotask(() => {
          state.readyState = CLOSED;
          this.dispatchEvent(new Event("error"));
          this.dispatchEvent(new CloseEvent("close", { code: 1006, reason: "", wasClean: false }));
        });
        return;
      }
      state.readyState = CLOSING;
      op.close(state.id, code ?? null, reason);
    }

    get [Symbol.toStringTag]() { return "WebSocket"; }
  }

  for (const [name, value] of [["CONNECTING", CONNECTING], ["OPEN", OPEN], ["CLOSING", CLOSING], ["CLOSED", CLOSED]]) {
    const descriptor = { value, enumerable: true, writable: false, configurable: false };
    Object.defineProperty(WebSocket, name, descriptor);
    Object.defineProperty(WebSocket.prototype, name, descriptor);
  }

  for (const type of HANDLERS) {
    Object.defineProperty(WebSocket.prototype, `on${type}`, {
      enumerable: true,
      configurable: true,
      get() { return stateOf(this).handlers[type] ?? null; },
      set(value) {
        const state = stateOf(this);
        state.handlers[type] = typeof value === "function" ? value : null;
        if (state.listeners[type] === undefined) {
          state.listeners[type] = (event) => {
            const handler = state.handlers[type];
            if (handler !== null) handler.call(this, event);
          };
          this.addEventListener(type, state.listeners[type]);
        }
      },
    });
  }

  function header(request, name) { return request.headers.get(name); }

  function upgradeWebSocket(request, options) {
    const requestId = serveHook.requestId(request);
    if (requestId === undefined) {
      throw new TypeError("Velyr.upgradeWebSocket expects a Request received by Velyr.serve");
    }
    const connection = String(header(request, "connection") ?? "").toLowerCase().split(",").map((v) => v.trim());
    const key = header(request, "sec-websocket-key");
    if (
      String(header(request, "upgrade") ?? "").toLowerCase() !== "websocket" ||
      !connection.includes("upgrade") ||
      header(request, "sec-websocket-version") !== "13" ||
      key === null
    ) {
      throw new TypeError("The request is not a WebSocket upgrade request (Upgrade headers missing or invalid)");
    }
    const requested = String(header(request, "sec-websocket-protocol") ?? "").split(",").map((v) => v.trim()).filter(Boolean);
    let protocol = null;
    if (options?.protocol !== undefined) {
      protocol = String(options.protocol);
      if (!requested.includes(protocol)) {
        throw new TypeError(`The client did not offer the subprotocol "${protocol}"`);
      }
    }
    const id = op.upgrade(requestId, protocol);
    const socket = new WebSocket(ACCEPTED, id);
    const response = serveHook.switchingProtocols([
      ["upgrade", "websocket"],
      ["connection", "Upgrade"],
      ["sec-websocket-accept", op.accept_key(key)],
      ...(protocol === null ? [] : [["sec-websocket-protocol", protocol]]),
    ]);
    return { socket, response };
  }

  Object.defineProperty(global.Velyr, "upgradeWebSocket", { value: upgradeWebSocket, writable: false, configurable: true, enumerable: true });
  Object.defineProperty(global, "WebSocket", { value: WebSocket, writable: true, configurable: true, enumerable: false });
  Object.defineProperty(global, "CloseEvent", { value: CloseEvent, writable: true, configurable: true, enumerable: false });
})(globalThis);

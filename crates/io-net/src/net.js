// Velyr.connect / Velyr.listen on top of the private net ops. Evaluated once per isolate, after the
// runtime-ops bootstrap created `Velyr` and after web-streams installed its internal native hooks.
(function (global) {
  "use strict";

  const OPS = [
    "connect_begin", "connect_finish", "listen", "accept", "accept_take", "read", "write",
    "shutdown", "close", "set_nodelay", "set_keepalive",
  ];
  const op = {};
  for (const name of OPS) {
    op[name] = global[`op_net_${name}`];
    delete global[`op_net_${name}`];
  }
  const { createNativeReadable, createNativeWritable } = global[Symbol.for("velyr.internal.streams")];

  const READ_CHUNK = 16 * 1024;
  const DONE = Object.freeze({ done: true, value: undefined });

  const addr = ({ hostname, port }) => Object.freeze({ transport: "tcp", hostname, port });

  function parseOptions(options, defaultHostname) {
    if (options === null || typeof options !== "object") throw new TypeError("Velyr net: options must be an object");
    const { hostname = defaultHostname, port } = options;
    if (typeof hostname !== "string") throw new TypeError("Velyr net: hostname must be a string");
    if (!Number.isInteger(port) || port < 0 || port > 65535) throw new TypeError("Velyr net: port must be an integer from 0 to 65535");
    return { hostname, port };
  }

  function closeQuietly(id) {
    try {
      op.close(id);
    } catch {
      // Already closed.
    }
  }

  class Conn {
    #id;
    #closed = false;
    #readable;
    #writable;

    constructor(id, { local, remote }) {
      this.#id = id;
      Object.defineProperty(this, "localAddr", { value: addr(local), enumerable: true });
      Object.defineProperty(this, "remoteAddr", { value: addr(remote), enumerable: true });
    }

    async read(buffer) {
      if (!(buffer instanceof Uint8Array)) throw new TypeError("Conn.read expects a Uint8Array");
      if (buffer.length === 0) return 0;
      const chunk = await op.read(this.#id, buffer.length);
      if (chunk === null) return null;
      const bytes = new Uint8Array(chunk);
      buffer.set(bytes);
      return bytes.length;
    }

    async write(data) {
      return op.write(this.#id, data);
    }

    close() {
      if (this.#closed) return;
      this.#closed = true;
      op.close(this.#id);
    }

    static idOf(conn) {
      return conn.#id;
    }

    // Marks a Conn whose resource was handed to a layered protocol (startTls) as closed.
    static detach(conn) {
      conn.#closed = true;
    }

    setNoDelay(noDelay = true) {
      op.set_nodelay(this.#id, Boolean(noDelay));
    }

    setKeepAlive(keepAlive = true) {
      op.set_keepalive(this.#id, Boolean(keepAlive));
    }

    get readable() {
      return (this.#readable ??= createNativeReadable(this.#id, (id) => op.read(id, READ_CHUNK)));
    }

    get writable() {
      return (this.#writable ??= createNativeWritable(
        this.#id,
        async (id, chunk) => {
          await op.write(id, chunk);
        },
        // Closing (or aborting) the stream half-closes the socket so the peer sees EOF.
        async (id) => {
          try {
            await op.shutdown(id);
          } catch {
            // The connection was already closed.
          }
        },
      ));
    }
  }

  class Listener {
    #id;
    #closed = false;

    constructor({ id, hostname, port }) {
      this.#id = id;
      Object.defineProperty(this, "addr", { value: addr({ hostname, port }), enumerable: true });
    }

    close() {
      if (this.#closed) return;
      this.#closed = true;
      op.close(this.#id);
    }

    [Symbol.asyncIterator]() {
      return this;
    }

    async next() {
      if (this.#closed) return DONE;
      try {
        if ((await op.accept(this.#id)) === null) return DONE;
        const info = JSON.parse(op.accept_take(this.#id));
        return { done: false, value: new Conn(info.id, info) };
      } catch (error) {
        if (this.#closed && error?.code === "VELYR_BAD_RESOURCE") return DONE;
        throw error;
      }
    }

    // Leaving a for-await loop does not close the listener; call close().
    async return() {
      return DONE;
    }
  }

  // Internal, for layered protocols (io-tls): reachable only as globalThis[Symbol.for("velyr.internal.net")].
  Object.defineProperty(global, Symbol.for("velyr.internal.net"), {
    value: Object.freeze({
      createConn: (id, info) => new Conn(id, info),
      idOf: Conn.idOf,
      detach: Conn.detach,
      closeId: closeQuietly,
    }),
    enumerable: false,
    configurable: false,
    writable: false,
  });

  const api = {
    async connect(options) {
      const { hostname, port } = parseOptions(options);
      const id = op.connect_begin(hostname, port);
      try {
        return new Conn(id, JSON.parse(await op.connect_finish(id)));
      } catch (error) {
        closeQuietly(id);
        throw error;
      }
    },
    async listen(options) {
      const { hostname, port } = parseOptions(options, "0.0.0.0");
      return new Listener(JSON.parse(op.listen(hostname, port)));
    },
  };

  for (const [name, value] of Object.entries(api)) {
    Object.defineProperty(global.Velyr, name, { value, writable: false, configurable: true, enumerable: true });
  }
})(globalThis);

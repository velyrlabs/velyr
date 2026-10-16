// Velyr.connectTls / listenTls / startTls on top of the private tls ops and io-net's Conn. Evaluated
// once per isolate, after the net bootstrap (it needs Velyr.listen and the internal net hook).
(function (global) {
  "use strict";

  const OPS = [
    "connect_begin", "connect_finish", "start_begin", "start_finish", "server_config",
    "config_release", "accept_finish",
  ];
  const op = {};
  for (const name of OPS) {
    op[name] = global[`op_tls_${name}`];
    delete global[`op_tls_${name}`];
  }
  const net = global[Symbol.for("velyr.internal.net")];
  const { listen } = global.Velyr;
  const decoder = new TextDecoder();
  const DONE = Object.freeze({ done: true, value: undefined });

  const pem = (value, what) => {
    if (typeof value === "string") return value;
    if (value instanceof Uint8Array || value instanceof ArrayBuffer) return decoder.decode(value);
    throw new TypeError(`Velyr tls: ${what} must be a PEM string or bytes`);
  };

  const list = (value, what) => {
    if (value === undefined) return [];
    if (!Array.isArray(value)) throw new TypeError(`Velyr tls: ${what} must be an array`);
    return value;
  };

  // null means "use the default" (10s).
  function timeoutMs(value) {
    if (value === undefined || value === null) return null;
    if (typeof value !== "number" || !(value > 0) || !Number.isFinite(value)) throw new TypeError("Velyr tls: handshakeTimeoutMs must be a positive number");
    return value;
  }

  function clientOptions({ hostname, caCerts, alpnProtocols, handshakeTimeoutMs }) {
    if (typeof hostname !== "string" || hostname === "") throw new TypeError("Velyr tls: hostname must be a non-empty string");
    return JSON.stringify({
      hostname,
      caCerts: list(caCerts, "caCerts").map((cert) => pem(cert, "caCerts entry")),
      alpnProtocols: list(alpnProtocols, "alpnProtocols").map(String),
      handshakeTimeoutMs: timeoutMs(handshakeTimeoutMs),
    });
  }

  function tlsConn(id, info) {
    const { alpnProtocol, ...addrs } = info;
    const conn = net.createConn(id, addrs);
    Object.defineProperty(conn, "alpnProtocol", { value: alpnProtocol, enumerable: true });
    return conn;
  }

  function checkOptions(options) {
    if (options === null || typeof options !== "object") throw new TypeError("Velyr tls: options must be an object");
    return options;
  }

  // Moves `plain`'s socket into a placeholder; on failure the placeholder (and the socket) is closed.
  async function upgrade(plain, finish) {
    const newId = op.start_begin(net.idOf(plain));
    net.detach(plain);
    try {
      return tlsConn(newId, JSON.parse(await finish(newId)));
    } catch (error) {
      net.closeId(newId);
      throw error;
    }
  }

  // Handshakes run concurrently with accepting, so a slow client cannot stall the listener; a client
  // that fails or times out is dropped. The accept loop starts on the first next().
  class TlsListener {
    #inner;
    #configId;
    #timeout;
    #closed = false;
    #started = false;
    #ready = [];
    #waiters = [];
    #ended = false;

    constructor(inner, configId, timeout) {
      this.#inner = inner;
      this.#configId = configId;
      this.#timeout = timeout;
      Object.defineProperty(this, "addr", { value: inner.addr, enumerable: true });
    }

    close() {
      if (this.#closed) return;
      this.#closed = true;
      this.#inner.close();
      op.config_release(this.#configId);
    }

    [Symbol.asyncIterator]() {
      return this;
    }

    #deliver(result) {
      const waiter = this.#waiters.shift();
      if (waiter) waiter(result);
      else this.#ready.push(result);
    }

    #end() {
      this.#ended = true;
      for (const waiter of this.#waiters.splice(0)) waiter({ done: true, value: undefined });
    }

    async #acceptLoop() {
      for (;;) {
        let accepted;
        try {
          accepted = await this.#inner.next();
        } catch (error) {
          // An accept error (sockets quota, EMFILE, ECONNABORTED) does not end the listener: report it
          // and park the loop; the next next() resumes accepting, so a persistent failure cannot spin.
          if (this.#closed) return this.#end();
          this.#started = false;
          return this.#deliver({ error });
        }
        if (accepted.done) return this.#end();
        upgrade(accepted.value, (id) => op.accept_finish(id, this.#configId, this.#timeout)).then(
          (conn) => this.#deliver({ done: false, value: conn }),
          (error) => {
            if (error?.code !== "VELYR_TLS" && error?.code !== "VELYR_TIMEOUT") this.#deliver({ error });
          },
        );
      }
    }

    async next() {
      if (!this.#started) {
        this.#started = true;
        this.#acceptLoop();
      }
      const result = this.#ready.length > 0 ? this.#ready.shift() : this.#ended ? DONE : await new Promise((resolve) => this.#waiters.push(resolve));
      if (result.error) throw result.error;
      return result;
    }

    async return() {
      return DONE;
    }
  }

  const api = {
    async connectTls(options) {
      const { hostname, port } = checkOptions(options);
      const json = clientOptions(options);
      const id = op.connect_begin(hostname, port);
      try {
        return tlsConn(id, JSON.parse(await op.connect_finish(id, json)));
      } catch (error) {
        net.closeId(id);
        throw error;
      }
    },
    async startTls(conn, options) {
      const json = clientOptions(checkOptions(options));
      return upgrade(conn, (id) => op.start_finish(id, json));
    },
    async listenTls(options) {
      const { hostname = "0.0.0.0", port, cert, key, alpnProtocols, handshakeTimeoutMs } = checkOptions(options);
      const timeout = timeoutMs(handshakeTimeoutMs);
      const configId = op.server_config(pem(cert, "cert"), pem(key, "key"), JSON.stringify(list(alpnProtocols, "alpnProtocols").map(String)));
      try {
        return new TlsListener(await listen({ hostname, port }), configId, timeout);
      } catch (error) {
        op.config_release(configId);
        throw error;
      }
    },
  };

  for (const [name, value] of Object.entries(api)) {
    Object.defineProperty(global.Velyr, name, { value, writable: false, configurable: true, enumerable: true });
  }
})(globalThis);

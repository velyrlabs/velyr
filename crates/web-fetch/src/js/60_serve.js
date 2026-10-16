
  // ---- Velyr.serve -----------------------------------------------------------------------

  const SERVE_OPS = ["listen", "accept", "take", "read", "respond", "write", "end", "fail", "shutdown", "close", "disconnected"];
  const serveOp = {};
  for (const name of SERVE_OPS) {
    serveOp[name] = global[`op_serve_${name}`];
    delete global[`op_serve_${name}`];
  }

  const NO_BODY_METHODS = ["GET", "HEAD"];
  // Request -> id of its serve resource, for layers that take over the connection (WebSocket).
  const serveRequestIds = new WeakMap();

  function serveRequest(rid, info, base, signal) {
    const headers = new Headers();
    for (const [name, value] of info.headers) headersAppendRaw(headers, name, value);
    headersGuard(headers, "request");
    const body = NO_BODY_METHODS.includes(info.method)
      ? null
      : nativeBody(rid, serveOp.read, undefined);
    let url;
    try {
      url = new URL(`${base.scheme}://${headers.get("host") ?? base.authority}${info.uri}`).href;
    } catch {
      url = new URL(`${base.scheme}://${base.authority}${info.uri}`).href;
    }
    const request = new Request(INTERNAL, {
      url, method: info.method, headers, signal, redirect: "follow", options: { ...DEFAULT_OPTIONS }, body,
    });
    serveRequestIds.set(request, rid);
    return request;
  }

  // Largest batch of ready chunks sent as one write.
  const MAX_WRITE_BYTES = 64 * 1024;
  const NOT_YET = Promise.resolve(Symbol("not yet"));

  // Writes the body of a streamed response. Every write is a round trip through the event loop and
  // a socket flush, so chunks the stream has ready right now (or yields within the same microtask
  // burst) leave together; a chunk that needs a later turn is never held back for them. Order is
  // kept, and bytes read before an error are still sent before it is raised. The last write carries the
  // end of the body when the batch reached it; the result says whether it did.
  async function serveStreamBody(rid, reader) {
    let pending = reader.read();
    for (;;) {
      const batch = [];
      let size = 0, ended = false, failure = null;
      let result = await pending;
      pending = null;
      for (;;) {
        if (result.done) { ended = true; break; }
        if (!(result.value instanceof Uint8Array)) {
          failure = new TypeError("Received a response body chunk that is not a Uint8Array");
          break;
        }
        batch.push(result.value);
        size += result.value.byteLength;
        if (size >= MAX_WRITE_BYTES) break;
        const read = reader.read();
        try {
          result = await Promise.race([read, NOT_YET]);
        } catch (error) {
          failure = error;
          break;
        }
        if (typeof result === "symbol") { pending = read; break; }
      }
      const folded = ended && failure === null && batch.length > 0;
      if (batch.length > 0) await serveOp.write(rid, batch.length === 1 ? batch[0] : concatChunks(batch, size), folded);
      if (failure !== null) throw failure;
      if (ended) return folded;
      pending ??= reader.read();
    }
  }

  function concatChunks(chunks, size) {
    const out = new Uint8Array(size);
    let at = 0;
    for (const chunk of chunks) {
      out.set(chunk, at);
      at += chunk.byteLength;
    }
    return out;
  }

  // Sends the handler's Response: head first, then the body piece by piece when it is a stream.
  async function serveResponse(rid, request, response) {
    if (!(response instanceof Response)) throw new TypeError("The handler must return a Response");
    if (response.status < 200 && response.status !== 101) throw new TypeError("The handler returned a Response that cannot be sent");
    const state = bodyStateOf(response);
    const pairs = JSON.stringify(headerPairs(response.headers));
    const empty = state === null || request.method === "HEAD" || NULL_BODY_STATUSES.includes(response.status);
    if (empty) return serveOp.respond(rid, response.status, pairs, null);
    if (state.unusable) throw new TypeError("The response body was already read");
    state.used = true;
    if (state.text !== null) return serveOp.respond(rid, response.status, pairs, state.text);
    if (state.bytes !== null) return serveOp.respond(rid, response.status, pairs, state.bytes);
    serveOp.respond(rid, response.status, pairs, true);
    const reader = state.stream.getReader();
    try {
      if (!(await serveStreamBody(rid, reader))) await serveOp.end(rid);
    } catch (error) {
      reader.cancel(error).catch(() => {});
      throw error;
    }
  }

  // Never rejects: a failing handler is a logged 500, and the request resource always closes.
  async function serveDispatch(info, context) {
    const rid = info.id;
    const controller = new AbortController();
    try {
      const request = serveRequest(rid, info, context.base, controller.signal);
      // Settles when the client leaves, or false once `close(rid)` below ends the request.
      serveOp.disconnected(rid).then((gone) => { if (gone) controller.abort(); }, () => {});
      const remoteAddr = info.remote === null ? null : { transport: "tcp", ...info.remote };
      await serveResponse(rid, request, await context.handler(request, { remoteAddr }));
    } catch (error) {
      console.error("Velyr.serve: the request handler failed:", error);
      controller.abort(error);
      try {
        serveOp.fail(rid);
      } catch {
        // Already answered or closed.
      }
    } finally {
      try {
        serveOp.close(rid);
      } catch {
        // Already closed.
      }
    }
  }

  // Runs until the server stopped and every connection finished, so requests in flight can still
  // be answered during the grace period.
  async function serveLoop(id, context) {
    const inflight = new Set();
    try {
      while ((await serveOp.accept(id)) !== null) {
        const info = JSON.parse(serveOp.take(id));
        if (info.id === null) continue;
        const running = serveDispatch(info, context).finally(() => inflight.delete(running));
        inflight.add(running);
      }
    } finally {
      await Promise.all(inflight);
      serveOp.close(id);
    }
  }

  class Server {
    #id;
    #done = false;

    constructor(id, addr, loop) {
      this.#id = id;
      Object.defineProperty(this, "addr", { value: addr, enumerable: true });
      Object.defineProperty(this, "finished", { value: loop.finally(() => { this.#done = true; }), enumerable: true });
    }

    // Stops accepting; requests in flight get until the grace period ends. Resolves like `finished`.
    shutdown() {
      if (!this.#done) serveOp.shutdown(this.#id);
      return this.finished;
    }
  }
  defineTag(Server, "Server");

  function serve(options, handler) {
    if (options === null || typeof options !== "object") throw new TypeError("Velyr.serve: options must be an object");
    if (typeof handler !== "function") throw new TypeError("Velyr.serve: the handler must be a function");
    const { hostname = "0.0.0.0", port = 8000, signal, tls, onListen, gracePeriod, inherit } = options;
    if (typeof hostname !== "string") throw new TypeError("Velyr.serve: hostname must be a string");
    if (!Number.isInteger(port) || port < 0 || port > 65535) throw new TypeError("Velyr.serve: port must be an integer from 0 to 65535");
    if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError("Velyr.serve: signal must be an AbortSignal");
    if (onListen !== undefined && typeof onListen !== "function") throw new TypeError("Velyr.serve: onListen must be a function");
    if (gracePeriod !== undefined && !(Number.isFinite(gracePeriod) && gracePeriod >= 0)) {
      throw new TypeError("Velyr.serve: gracePeriod must be a number of milliseconds");
    }
    if (tls !== undefined && (tls === null || typeof tls.cert !== "string" || typeof tls.key !== "string")) {
      throw new TypeError("Velyr.serve: tls needs cert and key as PEM strings");
    }
    const bound = JSON.parse(serveOp.listen(JSON.stringify({
      hostname, port, cert: tls?.cert, key: tls?.key, graceMs: gracePeriod === undefined ? undefined : Math.floor(gracePeriod),
      inherit: inherit === true,
    })));
    const host = bound.hostname.includes(":") ? `[${bound.hostname}]` : bound.hostname;
    const context = { handler, base: { scheme: tls === undefined ? "http" : "https", authority: `${host}:${bound.port}` } };
    const server = new Server(bound.id, { transport: "tcp", hostname: bound.hostname, port: bound.port }, serveLoop(bound.id, context));
    const stop = () => server.shutdown();
    if (signal !== undefined) {
      if (signal.aborted) stop();
      else {
        signal.addEventListener("abort", stop, { once: true });
        server.finished.then(() => signal.removeEventListener("abort", stop), () => signal.removeEventListener("abort", stop));
      }
    }
    try {
      onListen?.({ hostname: server.addr.hostname, port: server.addr.port });
    } catch (error) {
      stop();
      throw error;
    }
    return server;
  }

  Object.defineProperty(global.Velyr, "serve", { value: serve, writable: false, configurable: true, enumerable: true });

  // Reachable only as globalThis[Symbol.for("velyr.internal.serve")] (non-enumerable): lets the
  // WebSocket layer find a request's resource id and build the 101 response the handler returns.
  Object.defineProperty(global, Symbol.for("velyr.internal.serve"), {
    value: Object.freeze({
      requestId: (request) => serveRequestIds.get(request),
      switchingProtocols(pairs) {
        const headers = new Headers();
        for (const [name, value] of pairs) headersAppendRaw(headers, name, value);
        headersGuard(headers, "immutable");
        return new Response(INTERNAL, {
          type: "default", url: "", redirected: false, status: 101, statusText: "Switching Protocols",
          headers, guard: "immutable", body: null,
        });
      },
    }),
    enumerable: false,
    configurable: false,
    writable: false,
  });

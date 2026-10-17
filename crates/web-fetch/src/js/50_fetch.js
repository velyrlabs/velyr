
  // ---- fetch -----------------------------------------------------------------------------

  const MAX_REDIRECTS = 20;
  const CROSS_ORIGIN_STRIPPED = ["authorization", "cookie", "proxy-authorization"];

  function percentDecode(text) {
    const input = encoder.encode(text);
    const out = [];
    for (let i = 0; i < input.length; i++) {
      const hex = input[i] === 0x25 ? String.fromCharCode(input[i + 1] ?? 0, input[i + 2] ?? 0) : "";
      if (/^[0-9a-fA-F]{2}$/.test(hex)) {
        out.push(parseInt(hex, 16));
        i += 2;
      } else {
        out.push(input[i]);
      }
    }
    return Uint8Array.from(out);
  }

  function dataResponse(url) {
    const href = url.href.replace(/#.*$/s, "");
    const comma = href.indexOf(",");
    if (comma === -1) throw new TypeError("Invalid data: URL");
    let mime = href.slice(5, comma).trim();
    let bytes = percentDecode(href.slice(comma + 1));
    if (/;\s*base64$/i.test(mime)) {
      mime = mime.replace(/;\s*base64$/i, "");
      try {
        bytes = Uint8Array.from(atob(decoder.decode(bytes).replace(/[\t\n\f\r ]/g, "")), (c) => c.charCodeAt(0));
      } catch {
        throw new TypeError("Invalid base64 in data: URL");
      }
    }
    if (mime.startsWith(";")) mime = `text/plain${mime}`;
    if (mime === "") mime = "text/plain;charset=US-ASCII";
    return fetchedResponse({
      type: "basic", url: href, redirected: false, status: 200, statusText: "OK",
      pairs: [["content-type", mime]], body: new BodyState(null, bytes),
    });
  }

  // A blob: URL answers with its Blob while it is registered; a revoked or unknown one is a network error.
  function blobResponse(url, request) {
    const blob = blobInternal.objectURL?.(url.href);
    if (blob === undefined || request.method !== "GET") throw new TypeError("blob: URL is revoked, unknown or not fetched with GET");
    const pairs = [["content-length", String(blob.size)]];
    if (blob.type !== "") pairs.push(["content-type", blob.type]);
    return fetchedResponse({
      type: "basic", url: url.href, redirected: false, status: 200, statusText: "OK",
      pairs, body: new BodyState(null, blobInternal.readBytes(blob)),
    });
  }

  // The error a caller sees for a failed op: the abort reason if there is one, permission and
  // type errors untouched, anything else as the standard's network error.
  function failure(error, signal, message) {
    if (signal.aborted) return signal.reason;
    if (error instanceof TypeError || error?.code === "VELYR_PERMISSION_DENIED") return error;
    return new TypeError(message, { cause: error });
  }

  // Feeds a streamed request body to the connection. It outlives the response head, and even the
  // response body: the upload ends when the stream does, fails, or the fetch is aborted. A failure
  // after the head (the server stopped reading, the source errored) cancels the source stream
  // with that error instead of rejecting the fetch.
  function pumpRequestBody(rid, stream, state) {
    (async () => {
      const reader = stream.getReader();
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          if (!(value instanceof Uint8Array)) throw new TypeError("Received a request body chunk that is not a Uint8Array");
          await op.write(rid, value);
        }
        await op.end(rid);
      } catch (error) {
        reader.cancel(error).catch(() => {});
        if (!state.responded) {
          state.pumpFailure ??= error;
          closeQuietly(rid);
          return;
        }
        // The response is out: only the upload ends here, the response body stays readable.
        try {
          op.abort_upload(rid).catch(() => {});
        } catch {
          // The request is already gone.
        }
      }
    })();
  }

  // Closes whatever the request currently holds: the request id, or the body handle after detach.
  function closeState(state) {
    if (state.rid === null) return;
    const id = state.rid;
    state.rid = null;
    try {
      // Hard: an abort or a failed fetch also ends an upload that is still running.
      if (state.detached) op.release(id, true);
      else op.close(id);
    } catch {
      // Already closed.
    }
  }

  // Bodies nobody read to the end are released when their stream is garbage collected, where the
  // engine has FinalizationRegistry; otherwise (and for bodies still alive) when the isolate ends.
  const abandoned = typeof FinalizationRegistry === "function"
    ? new FinalizationRegistry((id) => {
      try {
        op.release(id);
      } catch {
        // Already released.
      }
    })
    : null;

  function wireHeaders(headers) {
    const pairs = headerPairs(headers);
    if (!headers.has("user-agent")) pairs.push(["user-agent", USER_AGENT]);
    if (!headers.has("accept")) pairs.push(["accept", "*/*"]);
    if (!headers.has("accept-encoding")) pairs.push(["accept-encoding", "gzip, deflate, br"]);
    return JSON.stringify(pairs);
  }

  // One request/response exchange. `begin` throws permission errors untouched.
  async function exchange(state, url, method, headers, body) {
    state.signal.throwIfAborted();
    const target = url.href.replace(/#.*$/s, "");
    const rid = op.begin(target);
    state.rid = rid;
    state.detached = false;
    state.responded = false;
    state.pumpFailure = undefined;
    try {
      const streamed = body !== null && body.bytes === null;
      const sending = op.send(rid, method, target, wireHeaders(headers), streamed ? true : body?.bytes ?? null, currentSpanId());
      if (streamed) pumpRequestBody(rid, body.stream, state);
      const head = JSON.parse(await sending);
      state.responded = true;
      return { rid, head };
    } catch (error) {
      closeQuietly(rid);
      state.rid = null;
      throw failure(state.pumpFailure ?? error, state.signal, "fetch failed");
    }
  }

  // The body of the final response: leaves the request id for a body handle (`op_fetch_detach`).
  function networkBody(rid, state, release) {
    const { signal } = state;
    const handle = op.detach(rid);
    state.rid = handle;
    state.detached = true;
    const finish = () => {
      if (state.rid === handle) state.rid = null;
      try {
        op.release(handle);
      } catch {
        // Already released.
      }
      release();
    };
    const body = nativeBody(
      handle,
      async (id) => {
        try {
          const chunk = await op.read(id);
          if (chunk === null) finish();
          return chunk;
        } catch (error) {
          finish();
          throw failure(error, signal, "terminated");
        }
      },
      finish,
    );
    abandoned?.register(body.stream, handle);
    return body;
  }

  function nextHop(request, status, url, location, current) {
    let next;
    try {
      next = new URL(location, url);
    } catch {
      throw new TypeError("The redirect location is not a valid URL");
    }
    if (next.protocol !== "http:" && next.protocol !== "https:") throw new TypeError("Redirect to an unsupported scheme");
    if (next.username !== "" || next.password !== "") throw new TypeError("Redirect URL must not include credentials");
    let { method, body } = current;
    const { headers } = current;
    const toGet = (status === 303 && method !== "GET" && method !== "HEAD") || ((status === 301 || status === 302) && method === "POST");
    if (toGet) {
      method = "GET";
      body = null;
      for (const name of REQUEST_BODY_HEADERS) headers.delete(name);
    }
    if (body !== null && body.bytes === null) throw new TypeError("Cannot follow a redirect with a streamed request body");
    if (next.origin !== url.origin) for (const name of CROSS_ORIGIN_STRIPPED) headers.delete(name);
    return { url: next, method, body, headers };
  }

  async function fetch(input, init = undefined) {
    if (arguments.length === 0) throw new TypeError("fetch requires 1 argument");
    const request = new Request(input, init);
    const { signal } = request;
    signal.throwIfAborted();
    let url = new URL(request.url);
    if (url.protocol === "data:") return dataResponse(url);
    if (url.protocol === "blob:") return blobResponse(url, request);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      throw new TypeError(`URL scheme "${url.protocol.slice(0, -1)}" is not supported`);
    }
    const first = bodies.get(request);
    if (first !== null) {
      if (first.unusable) throw new TypeError("Request body was already read");
      first.used = true;
    }
    let hop = { url, method: request.method, body: first, headers: new Headers(request.headers) };
    const state = { signal, rid: null, responded: false, pumpFailure: undefined };
    const onAbort = () => closeState(state);
    const release = () => signal.removeEventListener("abort", onAbort);
    signal.addEventListener("abort", onAbort, { once: true });
    let redirects = 0;
    try {
      for (;;) {
        ({ url } = hop);
        const { rid, head } = await exchange(state, url, hop.method, hop.headers, hop.body);
        const pairs = head.headers;
        const location = pairs.find(([name]) => name === "location")?.[1];
        if (REDIRECT_STATUSES.includes(head.status) && location !== undefined) {
          closeQuietly(rid);
          state.rid = null;
          if (request.redirect === "error") throw new TypeError("Redirect was refused: the redirect mode is 'error'");
          if (request.redirect === "manual") {
            release();
            return fetchedResponse({
              type: "opaqueredirect", url: url.href, redirected: redirects > 0, status: 0, statusText: "",
              pairs: [], body: null,
            });
          }
          if (++redirects > MAX_REDIRECTS) throw new TypeError("Too many redirects");
          hop = nextHop(request, head.status, url, location, hop);
          continue;
        }
        const empty = hop.method === "HEAD" || NULL_BODY_STATUSES.includes(head.status);
        if (empty) {
          closeQuietly(rid);
          release();
        }
        return fetchedResponse({
          type: "basic", url: url.href, redirected: redirects > 0, status: head.status,
          statusText: head.statusText, pairs, body: empty ? null : networkBody(rid, state, release),
        });
      }
    } catch (error) {
      closeState(state);
      release();
      throw error;
    }
  }

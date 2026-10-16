
  // ---- Response --------------------------------------------------------------------------

  function checkStatus(value) {
    const status = Number(value);
    if (!Number.isInteger(status) || status < 200 || status > 599) {
      throw new RangeError(`The status provided (${value}) is outside the range [200, 599]`);
    }
    return status;
  }

  class Response {
    #type = "default";
    #url = "";
    #redirected = false;
    #status = 200;
    #statusText = "";
    #headers;
    #guard = "response";

    constructor(body = null, init = undefined) {
      if (body === INTERNAL) {
        const fields = init;
        this.#type = fields.type;
        this.#url = fields.url;
        this.#redirected = fields.redirected;
        this.#status = fields.status;
        this.#statusText = fields.statusText;
        this.#headers = fields.headers;
        this.#guard = fields.guard;
        bodies.set(this, fields.body);
        return;
      }
      if (init !== undefined && init !== null && typeof init !== "object" && typeof init !== "function") {
        throw new TypeError("Response init must be an object");
      }
      const dict = init ?? {};
      if (dict.status !== undefined) this.#status = checkStatus(dict.status);
      if (dict.statusText !== undefined) {
        this.#statusText = String(dict.statusText);
        if (!REASON_PHRASE.test(this.#statusText)) throw new TypeError("Invalid statusText");
      }
      this.#headers = headersOf(dict.headers, "response");
      let state = null;
      let contentType = null;
      if (body !== null && body !== undefined) {
        if (NULL_BODY_STATUSES.includes(this.#status)) {
          throw new TypeError(`Response with null body status cannot have a body`);
        }
        [state, contentType] = extractBody(body);
      }
      if (contentType !== null && !this.#headers.has("content-type")) this.#headers.append("content-type", contentType);
      bodies.set(this, state);
    }

    static error() {
      return new Response(INTERNAL, {
        type: "error", url: "", redirected: false, status: 0, statusText: "",
        headers: headersOf(undefined, "immutable"), guard: "immutable", body: null,
      });
    }

    static json(data, init = undefined) {
      const text = JSON.stringify(data);
      if (text === undefined) throw new TypeError("The value is not JSON serializable");
      // Built bodiless, then given the text: a string body would default to text/plain.
      const response = new Response(null, init);
      if (NULL_BODY_STATUSES.includes(response.status)) {
        throw new TypeError(`Response with null body status cannot have a body`);
      }
      bodies.set(response, new BodyState(null, null, text));
      if (!response.headers.has("content-type")) response.headers.set("content-type", "application/json");
      return response;
    }

    static redirect(url, status = 302) {
      const location = new URL(String(url)).href;
      if (!REDIRECT_STATUSES.includes(Number(status))) throw new RangeError(`Invalid redirect status code ${status}`);
      const headers = headersOf(undefined, "none");
      headers.append("location", location);
      headersGuard(headers, "immutable");
      return new Response(INTERNAL, {
        type: "default", url: "", redirected: false, status: Number(status), statusText: "",
        headers, guard: "immutable", body: null,
      });
    }

    get type() { return this.#type; }
    get url() { return this.#url; }
    get redirected() { return this.#redirected; }
    get status() { return this.#status; }
    get ok() { return this.#status >= 200 && this.#status <= 299; }
    get statusText() { return this.#statusText; }
    get headers() { return this.#headers; }

    clone() {
      const state = bodyStateOf(this);
      if (state !== null && state.unusable) throw new TypeError("Cannot clone a Response whose body was read");
      return new Response(INTERNAL, {
        type: this.#type, url: this.#url, redirected: this.#redirected, status: this.#status,
        statusText: this.#statusText, headers: headersOf(this.#headers, this.#guard), guard: this.#guard,
        body: cloneBody(state),
      });
    }
  }
  defineTag(Response, "Response");
  mixBody(Response);
  enumerable(Response, ["type", "url", "redirected", "status", "ok", "statusText", "headers", "clone"]);

  // What fetch() hands back: immutable headers, a stream over the network body.
  function fetchedResponse({ type, url, redirected, status, statusText, pairs, body }) {
    const headers = new Headers();
    for (const [name, value] of pairs) headersAppendRaw(headers, name, value);
    headersGuard(headers, "immutable");
    return new Response(INTERNAL, {
      type, url, redirected, status, statusText, headers, guard: "immutable", body,
    });
  }

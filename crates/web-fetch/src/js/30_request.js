
  // ---- Request ---------------------------------------------------------------------------

  // Lets fetch() and clone() build instances from prepared fields; user code cannot name it.
  const INTERNAL = Symbol("velyr.fetch.internal");
  const FORBIDDEN_METHODS = ["CONNECT", "TRACE", "TRACK"];
  const NORMALIZED_METHODS = ["DELETE", "GET", "HEAD", "OPTIONS", "POST", "PUT"];
  const REQUEST_BODY_HEADERS = ["content-encoding", "content-language", "content-location", "content-type", "content-length"];
  const DEFAULT_OPTIONS = {
    cache: "default",
    credentials: "same-origin",
    integrity: "",
    keepalive: false,
    mode: "cors",
    referrer: "about:client",
    referrerPolicy: "",
  };

  function parseRequestUrl(text) {
    const url = new URL(text);
    if (url.username !== "" || url.password !== "") {
      throw new TypeError("Request URL must not include credentials");
    }
    return url.href;
  }

  function checkMethod(value) {
    const method = String(value);
    if (!TOKEN.test(method)) throw new TypeError(`'${method}' is not a valid HTTP method`);
    const upper = method.toUpperCase();
    if (FORBIDDEN_METHODS.includes(upper)) throw new TypeError(`'${method}' HTTP method is unsupported`);
    return NORMALIZED_METHODS.includes(upper) ? upper : method;
  }

  // A signal that aborts with `source`; an unrelated one when there is none.
  function followSignal(source) {
    const controller = new AbortController();
    if (source === undefined || source === null) return controller.signal;
    if (!(source instanceof AbortSignal)) throw new TypeError("Request signal must be an AbortSignal");
    if (source.aborted) controller.abort(source.reason);
    else source.addEventListener("abort", () => controller.abort(source.reason), { once: true });
    return controller.signal;
  }

  function checkOptions(init, base) {
    const options = { ...base };
    for (const key of Object.keys(DEFAULT_OPTIONS)) {
      if (init[key] === undefined) continue;
      options[key] = key === "keepalive" ? Boolean(init[key]) : String(init[key]);
    }
    requireEnum(options.credentials, ["omit", "same-origin", "include"], "credentials");
    requireEnum(options.cache, ["default", "no-store", "reload", "no-cache", "force-cache", "only-if-cached"], "cache");
    requireEnum(options.mode, ["same-origin", "cors", "no-cors"], "mode");
    return options;
  }

  class Request {
    #url;
    #method;
    #headers;
    #signal;
    #redirect;
    #options;

    constructor(input, init = undefined) {
      if (arguments.length === 0) throw new TypeError("Request requires 1 argument");
      if (input === INTERNAL) {
        const fields = init;
        this.#url = fields.url;
        this.#method = fields.method;
        this.#headers = fields.headers;
        this.#signal = fields.signal;
        this.#redirect = fields.redirect;
        this.#options = fields.options;
        bodies.set(this, fields.body);
        return;
      }
      if (init !== undefined && init !== null && typeof init !== "object" && typeof init !== "function") {
        throw new TypeError("Request init must be an object");
      }
      const dict = init ?? {};
      let signal;
      let method = "GET";
      let redirect = "follow";
      let baseOptions = DEFAULT_OPTIONS;
      let headers;
      let inherited = null;
      if (input instanceof Request) {
        this.#url = input.#url;
        method = input.#method;
        redirect = input.#redirect;
        baseOptions = input.#options;
        headers = input.#headers;
        signal = input.#signal;
        inherited = bodies.get(input);
      } else {
        this.#url = parseRequestUrl(String(input));
      }
      if (dict.method !== undefined) method = checkMethod(dict.method);
      if (dict.redirect !== undefined) redirect = requireEnum(dict.redirect, ["follow", "error", "manual"], "redirect");
      if (dict.duplex !== undefined && String(dict.duplex) !== "half") {
        throw new TypeError("Request duplex must be 'half'");
      }
      this.#method = method;
      this.#redirect = redirect;
      this.#options = checkOptions(dict, baseOptions);
      this.#signal = followSignal(dict.signal !== undefined ? dict.signal : signal);
      this.#headers = headersOf(dict.headers !== undefined ? dict.headers : headers, "request");

      let state = null;
      let contentType = null;
      if (dict.body !== undefined && dict.body !== null) {
        [state, contentType] = extractBody(dict.body);
        if (state.stream !== null && dict.duplex === undefined) {
          throw new TypeError("The 'duplex' option is required when the body is a stream");
        }
      } else if (inherited !== null) {
        if (inherited.unusable) throw new TypeError("Cannot construct a Request from one whose body was read");
        state = new BodyState(inherited.stream, inherited.bytes);
        inherited.used = true;
      }
      if (state !== null && (method === "GET" || method === "HEAD")) {
        throw new TypeError("Request with GET/HEAD method cannot have a body");
      }
      if (contentType !== null && !this.#headers.has("content-type")) this.#headers.append("content-type", contentType);
      bodies.set(this, state);
    }

    get url() { return this.#url; }
    get method() { return this.#method; }
    get headers() { return this.#headers; }
    get destination() { return ""; }
    get referrer() { return this.#options.referrer; }
    get referrerPolicy() { return this.#options.referrerPolicy; }
    get mode() { return this.#options.mode; }
    get credentials() { return this.#options.credentials; }
    get cache() { return this.#options.cache; }
    get redirect() { return this.#redirect; }
    get integrity() { return this.#options.integrity; }
    get keepalive() { return this.#options.keepalive; }
    get duplex() { return "half"; }
    get signal() { return this.#signal; }

    clone() {
      const state = bodyStateOf(this);
      if (state !== null && state.unusable) throw new TypeError("Cannot clone a Request whose body was read");
      return new Request(INTERNAL, {
        url: this.#url,
        method: this.#method,
        headers: headersOf(this.#headers, "request"),
        signal: followSignal(this.#signal),
        redirect: this.#redirect,
        options: { ...this.#options },
        body: cloneBody(state),
      });
    }
  }
  defineTag(Request, "Request");
  mixBody(Request);
  enumerable(Request, [
    "url", "method", "headers", "destination", "referrer", "referrerPolicy", "mode", "credentials",
    "cache", "redirect", "integrity", "keepalive", "duplex", "signal", "clone",
  ]);

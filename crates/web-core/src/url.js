// Bootstrap: URL and URLSearchParams on top of the private op_url_* binding.
// Evaluated once per isolate. Parsing and setter semantics live in Rust (the `url` crate);
// this file keeps the component state and the URL <-> URLSearchParams sync.
(function (global) {
  "use strict";

  // Capture the raw ops, then remove them so user code never sees them (ADR-010).
  const parseOp = global.op_url_parse;
  const reparseOp = global.op_url_reparse;
  delete global.op_url_parse;
  delete global.op_url_reparse;

  // Indexes into the component array the ops return.
  const HREF = 0, ORIGIN = 1, PROTOCOL = 2, USERNAME = 3, PASSWORD = 4, HOST = 5;
  const HOSTNAME = 6, PORT = 7, PATHNAME = 8, SEARCH = 9, HASH = 10;

  function define(name, value) {
    Object.defineProperty(global, name, {
      value,
      writable: true,
      configurable: true,
      enumerable: false,
    });
  }

  const usv = (value) => String(value).toWellFormed();

  // ---- application/x-www-form-urlencoded ----

  function encodeForm(text) {
    return encodeURIComponent(usv(text))
      .replace(/[!'()~]/g, (c) => "%" + c.charCodeAt(0).toString(16).toUpperCase())
      .replace(/%20/g, "+");
  }

  // WHATWG UTF-8 decode: invalid sequences become U+FFFD instead of throwing.
  function decodeUtf8(bytes) {
    let out = "";
    let needed = 0, point = 0, lower = 0x80, upper = 0xbf;
    for (let i = 0; i < bytes.length; i++) {
      const byte = bytes[i];
      if (needed === 0) {
        if (byte <= 0x7f) out += String.fromCharCode(byte);
        else if (byte >= 0xc2 && byte <= 0xdf) { needed = 1; point = byte & 0x1f; }
        else if (byte >= 0xe0 && byte <= 0xef) {
          if (byte === 0xe0) lower = 0xa0;
          if (byte === 0xed) upper = 0x9f;
          needed = 2; point = byte & 0xf;
        } else if (byte >= 0xf0 && byte <= 0xf4) {
          if (byte === 0xf0) lower = 0x90;
          if (byte === 0xf4) upper = 0x8f;
          needed = 3; point = byte & 0x7;
        } else out += "�";
        continue;
      }
      if (byte < lower || byte > upper) {
        needed = 0; point = 0; lower = 0x80; upper = 0xbf;
        out += "�";
        i--;
        continue;
      }
      lower = 0x80; upper = 0xbf;
      point = (point << 6) | (byte & 0x3f);
      if (--needed === 0) { out += String.fromCodePoint(point); point = 0; }
    }
    return needed === 0 ? out : out + "�";
  }

  function decodeForm(text) {
    const spaced = text.replace(/\+/g, " ");
    return spaced.replace(/(?:%[0-9a-fA-F]{2})+/g, (run) => {
      const bytes = [];
      for (let i = 0; i < run.length; i += 3) bytes.push(parseInt(run.slice(i + 1, i + 3), 16));
      return decodeUtf8(bytes);
    });
  }

  function parseForm(input) {
    const list = [];
    for (const piece of input.split("&")) {
      if (piece === "") continue;
      const eq = piece.indexOf("=");
      const name = eq < 0 ? piece : piece.slice(0, eq);
      const value = eq < 0 ? "" : piece.slice(eq + 1);
      list.push([decodeForm(name), decodeForm(value)]);
    }
    return list;
  }

  // ---- URLSearchParams ----

  let attachParams, replaceParams;

  class URLSearchParams {
    #list = [];
    #onChange = null;

    static {
      attachParams = (params, onChange) => { params.#onChange = onChange; };
      replaceParams = (params, query) => { params.#list = parseForm(query); };
    }

    constructor(init = undefined) {
      if (init === undefined || init === null) return;
      if (typeof init === "object" || typeof init === "function") {
        if (typeof init[Symbol.iterator] === "function") {
          for (const pair of init) {
            const items = [...pair];
            if (items.length !== 2) {
              throw new TypeError("Each query pair must be an iterable [name, value] tuple");
            }
            this.#list.push([usv(items[0]), usv(items[1])]);
          }
        } else {
          for (const key of Reflect.ownKeys(init)) {
            if (typeof key === "string" && Object.getOwnPropertyDescriptor(init, key)?.enumerable) {
              this.#list.push([usv(key), usv(init[key])]);
            }
          }
        }
        return;
      }
      const text = usv(init);
      this.#list = parseForm(text.startsWith("?") ? text.slice(1) : text);
    }

    get size() { return this.#list.length; }

    append(name, value) {
      this.#list.push([usv(name), usv(value)]);
      this.#changed();
    }

    delete(name, value = undefined) {
      name = usv(name);
      const match = value === undefined
        ? (pair) => pair[0] === name
        : ((v) => (pair) => pair[0] === name && pair[1] === v)(usv(value));
      this.#list = this.#list.filter((pair) => !match(pair));
      this.#changed();
    }

    get(name) {
      name = usv(name);
      const pair = this.#list.find((p) => p[0] === name);
      return pair === undefined ? null : pair[1];
    }

    getAll(name) {
      name = usv(name);
      return this.#list.filter((p) => p[0] === name).map((p) => p[1]);
    }

    has(name, value = undefined) {
      name = usv(name);
      if (value === undefined) return this.#list.some((p) => p[0] === name);
      value = usv(value);
      return this.#list.some((p) => p[0] === name && p[1] === value);
    }

    set(name, value) {
      name = usv(name);
      value = usv(value);
      const first = this.#list.findIndex((p) => p[0] === name);
      if (first < 0) {
        this.#list.push([name, value]);
      } else {
        this.#list[first][1] = value;
        this.#list = this.#list.filter((p, i) => i <= first || p[0] !== name);
      }
      this.#changed();
    }

    sort() {
      // Array.prototype.sort is stable and compares strings by UTF-16 code unit, as required.
      this.#list.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
      this.#changed();
    }

    toString() {
      return this.#list.map(([n, v]) => encodeForm(n) + "=" + encodeForm(v)).join("&");
    }

    forEach(callback, thisArg = undefined) {
      if (typeof callback !== "function") {
        throw new TypeError("The callback provided as parameter 1 is not a function");
      }
      // Index loop: callbacks that mutate the list are observed, like the spec's live list.
      for (let i = 0; i < this.#list.length; i++) {
        Reflect.apply(callback, thisArg, [this.#list[i][1], this.#list[i][0], this]);
      }
    }

    // shortcut: iterators are generators, not distinct URLSearchParams Iterator objects.
    *entries() {
      for (let i = 0; i < this.#list.length; i++) yield [this.#list[i][0], this.#list[i][1]];
    }

    *keys() {
      for (let i = 0; i < this.#list.length; i++) yield this.#list[i][0];
    }

    *values() {
      for (let i = 0; i < this.#list.length; i++) yield this.#list[i][1];
    }

    [Symbol.iterator]() { return this.entries(); }

    get [Symbol.toStringTag]() { return "URLSearchParams"; }

    #changed() {
      if (this.#onChange !== null) this.#onChange(this.toString());
    }
  }

  // ---- URL ----

  class URL {
    #parts;
    #params;

    constructor(url, base = undefined) {
      const parts = parseOp(usv(url), base === undefined ? undefined : usv(base));
      if (parts === null) throw new TypeError("Invalid URL");
      this.#parts = parts;
      this.#params = new URLSearchParams(parts[SEARCH]);
      attachParams(this.#params, (query) => this.#apply("search", query, false));
    }

    static canParse(url, base = undefined) {
      return parseOp(usv(url), base === undefined ? undefined : usv(base)) !== null;
    }

    static parse(url, base = undefined) {
      try {
        return new URL(url, base);
      } catch {
        return null;
      }
    }

    #apply(setter, value, syncParams) {
      const parts = reparseOp(this.#parts[HREF], setter, usv(value));
      if (parts === null) throw new TypeError("Invalid URL");
      this.#parts = parts;
      if (syncParams) replaceParams(this.#params, parts[SEARCH].slice(1));
    }

    get href() { return this.#parts[HREF]; }
    set href(value) { this.#apply("href", value, true); }
    get origin() { return this.#parts[ORIGIN]; }
    get protocol() { return this.#parts[PROTOCOL]; }
    set protocol(value) { this.#apply("protocol", value, false); }
    get username() { return this.#parts[USERNAME]; }
    set username(value) { this.#apply("username", value, false); }
    get password() { return this.#parts[PASSWORD]; }
    set password(value) { this.#apply("password", value, false); }
    get host() { return this.#parts[HOST]; }
    set host(value) { this.#apply("host", value, false); }
    get hostname() { return this.#parts[HOSTNAME]; }
    set hostname(value) { this.#apply("hostname", value, false); }
    get port() { return this.#parts[PORT]; }
    set port(value) { this.#apply("port", value, false); }
    get pathname() { return this.#parts[PATHNAME]; }
    set pathname(value) { this.#apply("pathname", value, false); }
    get search() { return this.#parts[SEARCH]; }
    set search(value) { this.#apply("search", value, true); }
    get searchParams() { return this.#params; }
    get hash() { return this.#parts[HASH]; }
    set hash(value) { this.#apply("hash", value, false); }

    toString() { return this.#parts[HREF]; }
    toJSON() { return this.#parts[HREF]; }

    get [Symbol.toStringTag]() { return "URL"; }
  }

  define("URL", URL);
  define("URLSearchParams", URLSearchParams);
})(globalThis);

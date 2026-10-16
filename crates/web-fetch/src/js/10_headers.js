
  // ---- Headers ---------------------------------------------------------------------------

  let headersGuard;
  let headersMap;
  let headersAppendRaw;

  function normalizeValue(value) {
    const text = `${value}`.replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, "");
    if (/[\0\r\n]/.test(text)) throw new TypeError(`Invalid header value: ${JSON.stringify(text)}`);
    return text;
  }

  function checkName(name) {
    const text = `${name}`;
    if (!TOKEN.test(text)) throw new TypeError(`Invalid header name: ${JSON.stringify(text)}`);
    return text.toLowerCase();
  }

  class Headers {
    // lowercase name -> values, in the order they were added
    #map = new Map();
    #guard = "none";

    static {
      headersGuard = (headers, guard) => { headers.#guard = guard; };
      headersMap = (headers) => headers.#map;
      headersAppendRaw = (headers, name, value) => {
        const key = name.toLowerCase();
        const values = headers.#map.get(key);
        if (values === undefined) headers.#map.set(key, [value]);
        else values.push(value);
      };
    }

    constructor(init = undefined) {
      if (init === undefined) return;
      if (init === null || (typeof init !== "object" && typeof init !== "function")) {
        throw new TypeError("Headers init must be an object, an iterable of pairs or a Headers");
      }
      if (init instanceof Headers) {
        for (const [name, values] of init.#map) this.#map.set(name, [...values]);
      } else if (typeof init[Symbol.iterator] === "function") {
        for (const pair of init) {
          const entry = pair === null || typeof pair !== "object" ? undefined : [...pair];
          if (entry === undefined || entry.length !== 2) {
            throw new TypeError("Each header pair must be an iterable of exactly two items");
          }
          this.append(entry[0], entry[1]);
        }
      } else {
        for (const name of Reflect.ownKeys(init)) {
          if (typeof name === "string" && Object.getOwnPropertyDescriptor(init, name)?.enumerable) {
            this.append(name, init[name]);
          }
        }
      }
    }

    #mutable() {
      if (this.#guard === "immutable") throw new TypeError("Headers are immutable");
    }

    append(name, value) {
      if (arguments.length < 2) throw new TypeError("Headers.append requires 2 arguments");
      const key = checkName(name);
      const text = normalizeValue(value);
      this.#mutable();
      const values = this.#map.get(key);
      if (values === undefined) this.#map.set(key, [text]);
      else values.push(text);
    }

    delete(name) {
      if (arguments.length < 1) throw new TypeError("Headers.delete requires 1 argument");
      const key = checkName(name);
      this.#mutable();
      this.#map.delete(key);
    }

    get(name) {
      if (arguments.length < 1) throw new TypeError("Headers.get requires 1 argument");
      const values = this.#map.get(checkName(name));
      return values === undefined ? null : values.join(", ");
    }

    getSetCookie() {
      return [...(this.#map.get("set-cookie") ?? [])];
    }

    has(name) {
      if (arguments.length < 1) throw new TypeError("Headers.has requires 1 argument");
      return this.#map.has(checkName(name));
    }

    set(name, value) {
      if (arguments.length < 2) throw new TypeError("Headers.set requires 2 arguments");
      const key = checkName(name);
      const text = normalizeValue(value);
      this.#mutable();
      this.#map.set(key, [text]);
    }

    // Sorted by name; every Set-Cookie on its own, the rest combined.
    #sorted() {
      const pairs = [];
      for (const key of [...this.#map.keys()].sort()) {
        const values = this.#map.get(key);
        if (key === "set-cookie") for (const value of values) pairs.push([key, value]);
        else pairs.push([key, values.join(", ")]);
      }
      return pairs;
    }

    forEach(callback, thisArg = undefined) {
      if (typeof callback !== "function") throw new TypeError("Headers.forEach requires a function");
      for (const [name, value] of this.#sorted()) Reflect.apply(callback, thisArg, [value, name, this]);
    }

    // shortcut: iterators walk a snapshot taken when they are created, not the live list.
    keys() { return this.#sorted().map(([name]) => name)[Symbol.iterator](); }
    values() { return this.#sorted().map(([, value]) => value)[Symbol.iterator](); }
    entries() { return this.#sorted()[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
  }
  defineTag(Headers, "Headers");
  enumerable(Headers, ["append", "delete", "get", "getSetCookie", "has", "set", "forEach", "keys", "values", "entries"]);

  function headersOf(init, guard) {
    const headers = new Headers(init);
    headersGuard(headers, guard);
    return headers;
  }

  // [[name, value], ...] as the wire sees them: Set-Cookie separate, the rest combined.
  function headerPairs(headers) {
    return [...headers];
  }

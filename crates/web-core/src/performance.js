// Bootstrap: performance (now, timeOrigin, marks, measures, observer) and navigator on top of the
// private op_perf_* and op_navigator_* bindings. Evaluated once per isolate.
(function (global) {
  "use strict";

  // Capture the raw ops, then remove them so user code never sees them (ADR-010).
  const nowOp = global.op_perf_now;
  const timeOriginOp = global.op_perf_time_origin;
  const userAgentOp = global.op_navigator_user_agent;
  const hardwareConcurrencyOp = global.op_navigator_hardware_concurrency;
  delete global.op_perf_now;
  delete global.op_perf_time_origin;
  delete global.op_navigator_user_agent;
  delete global.op_navigator_hardware_concurrency;

  const ILLEGAL = Symbol("illegal constructor");
  const ENTRY_TYPES = ["mark", "measure"];
  const entries = [];
  const observers = new Set();

  function define(name, value) {
    Object.defineProperty(global, name, {
      value,
      writable: true,
      configurable: true,
      enumerable: false,
    });
  }

  function syntaxError(message) {
    const Ctor = global.DOMException;
    return typeof Ctor === "function" ? new Ctor(message, "SyntaxError") : new SyntaxError(message);
  }

  class PerformanceEntry {
    #name;
    #entryType;
    #startTime;
    #duration;
    constructor(token, name, entryType, startTime, duration) {
      if (token !== ILLEGAL) throw new TypeError("Illegal constructor");
      this.#name = name;
      this.#entryType = entryType;
      this.#startTime = startTime;
      this.#duration = duration;
    }
    get name() {
      return this.#name;
    }
    get entryType() {
      return this.#entryType;
    }
    get startTime() {
      return this.#startTime;
    }
    get duration() {
      return this.#duration;
    }
    toJSON() {
      return {
        name: this.name,
        entryType: this.entryType,
        startTime: this.startTime,
        duration: this.duration,
      };
    }
  }

  class PerformanceMark extends PerformanceEntry {
    #detail;
    constructor(name, options) {
      if (arguments.length === 0) {
        throw new TypeError("PerformanceMark requires at least 1 argument");
      }
      const opts = options ?? {};
      const startTime = opts.startTime === undefined ? nowOp() : Number(opts.startTime);
      if (!Number.isFinite(startTime) || startTime < 0) {
        throw new TypeError("PerformanceMark startTime must be a non-negative number");
      }
      super(ILLEGAL, String(name), "mark", startTime, 0);
      // shortcut: detail is stored by reference, the spec clones it; web-core has no hard dependency on structuredClone.
      this.#detail = opts.detail === undefined ? null : opts.detail;
    }
    get detail() {
      return this.#detail;
    }
  }

  class PerformanceMeasure extends PerformanceEntry {
    #detail;
    constructor(token, name, startTime, duration, detail) {
      super(token, name, "measure", startTime, duration);
      this.#detail = detail;
    }
    get detail() {
      return this.#detail;
    }
  }

  function record(entry) {
    entries.push(entry);
    for (const observer of observers) observer._enqueue(entry);
  }

  // A mark name resolves to the startTime of its most recent mark; a number is used as is.
  function resolveTimestamp(value) {
    if (typeof value === "number") return value;
    const name = String(value);
    for (let i = entries.length - 1; i >= 0; i--) {
      const entry = entries[i];
      if (entry.entryType === "mark" && entry.name === name) return entry.startTime;
    }
    throw syntaxError(`The mark '${name}' does not exist`);
  }

  function measureBounds(startOrOptions, endMark) {
    if (startOrOptions !== null && typeof startOrOptions === "object") {
      const { start, end, duration } = startOrOptions;
      const hasStart = start !== undefined;
      const hasEnd = end !== undefined;
      if (duration !== undefined && hasStart && hasEnd) {
        throw new TypeError("measure cannot set start, end and duration together");
      }
      let from = hasStart ? resolveTimestamp(start) : 0;
      let to = hasEnd ? resolveTimestamp(end) : nowOp();
      if (duration !== undefined) {
        if (hasStart && !hasEnd) to = from + Number(duration);
        else if (hasEnd && !hasStart) from = to - Number(duration);
      }
      return [from, to];
    }
    const from = startOrOptions === undefined ? 0 : resolveTimestamp(startOrOptions);
    const to = endMark === undefined ? nowOp() : resolveTimestamp(endMark);
    return [from, to];
  }

  function matches(entry, name, type) {
    return (name === undefined || entry.name === name) && (type === undefined || entry.entryType === type);
  }

  class Performance {
    constructor(token) {
      if (token !== ILLEGAL) throw new TypeError("Illegal constructor");
    }
    now() {
      return nowOp();
    }
    get timeOrigin() {
      return timeOriginOp();
    }
    mark(name, options) {
      if (arguments.length === 0) throw new TypeError("performance.mark requires a name");
      const entry = new PerformanceMark(name, options);
      record(entry);
      return entry;
    }
    measure(name, startOrOptions, endMark) {
      if (arguments.length === 0) throw new TypeError("performance.measure requires a name");
      const [start, end] = measureBounds(startOrOptions, endMark);
      const detail = startOrOptions?.detail;
      const entry = new PerformanceMeasure(
        ILLEGAL,
        String(name),
        start,
        end - start,
        detail === undefined ? null : detail,
      );
      record(entry);
      return entry;
    }
    getEntries() {
      return entries.slice();
    }
    getEntriesByName(name, type) {
      return entries.filter((entry) => matches(entry, String(name), type));
    }
    getEntriesByType(type) {
      return entries.filter((entry) => matches(entry, undefined, type));
    }
    clearMarks(name) {
      clearEntries("mark", name);
    }
    clearMeasures(name) {
      clearEntries("measure", name);
    }
    toJSON() {
      return { timeOrigin: this.timeOrigin };
    }
  }

  function clearEntries(type, name) {
    const keep = entries.filter((entry) => !(entry.entryType === type && (name === undefined || entry.name === String(name))));
    entries.length = 0;
    entries.push(...keep);
  }

  class PerformanceObserverEntryList {
    #entries;
    constructor(token, list) {
      if (token !== ILLEGAL) throw new TypeError("Illegal constructor");
      this.#entries = list;
    }
    getEntries() {
      return this.#entries.slice();
    }
    getEntriesByName(name, type) {
      return this.#entries.filter((entry) => matches(entry, String(name), type));
    }
    getEntriesByType(type) {
      return this.#entries.filter((entry) => matches(entry, undefined, type));
    }
  }

  class PerformanceObserver {
    #callback;
    #types = new Set();
    #queue = [];
    #scheduled = false;
    constructor(callback) {
      if (typeof callback !== "function") {
        throw new TypeError("PerformanceObserver callback must be a function");
      }
      this.#callback = callback;
      // Reached by record() only; not part of the Web API surface visible to user code.
      Object.defineProperty(this, "_enqueue", {
        value: (entry) => this.#enqueue(entry),
        enumerable: false,
      });
    }
    static get supportedEntryTypes() {
      return ENTRY_TYPES.slice();
    }
    observe(options) {
      if (options === null || typeof options !== "object") {
        throw new TypeError("PerformanceObserver.observe requires an options object");
      }
      const { entryTypes, type } = options;
      if (entryTypes !== undefined && type !== undefined) {
        throw new TypeError("observe cannot set both entryTypes and type");
      }
      const requested = entryTypes !== undefined ? Array.from(entryTypes, String) : type !== undefined ? [String(type)] : null;
      if (requested === null) throw new TypeError("observe requires entryTypes or type");
      this.#types = new Set(requested.filter((t) => ENTRY_TYPES.includes(t)));
      if (this.#types.size > 0) observers.add(this);
    }
    disconnect() {
      observers.delete(this);
      this.#queue = [];
    }
    takeRecords() {
      const taken = this.#queue;
      this.#queue = [];
      return taken;
    }
    #enqueue(entry) {
      if (!this.#types.has(entry.entryType)) return;
      this.#queue.push(entry);
      if (this.#scheduled) return;
      this.#scheduled = true;
      Promise.resolve().then(() => this.#flush());
    }
    #flush() {
      this.#scheduled = false;
      if (this.#queue.length === 0) return;
      const list = new PerformanceObserverEntryList(ILLEGAL, this.takeRecords());
      this.#callback.call(this, list, this);
    }
  }

  class Navigator {
    constructor(token) {
      if (token !== ILLEGAL) throw new TypeError("Illegal constructor");
    }
    get userAgent() {
      return userAgentOp();
    }
    get hardwareConcurrency() {
      return hardwareConcurrencyOp();
    }
    // shortcut: fixed locale, the web-core reads no environment; upgrade when a host locale source exists.
    get language() {
      return "en-US";
    }
    get languages() {
      return Object.freeze(["en-US"]);
    }
  }

  define("PerformanceEntry", PerformanceEntry);
  define("PerformanceMark", PerformanceMark);
  define("PerformanceMeasure", PerformanceMeasure);
  define("PerformanceObserver", PerformanceObserver);
  define("PerformanceObserverEntryList", PerformanceObserverEntryList);
  define("Performance", Performance);
  define("Navigator", Navigator);
  define("performance", new Performance(ILLEGAL));
  define("navigator", new Navigator(ILLEGAL));
})(globalThis);

const EventEmitter = node("events");
const { once, on, errorMonitor } = EventEmitter;
test("shape", () => {
  eq(EventEmitter.EventEmitter, EventEmitter);
  eq(EventEmitter.defaultMaxListeners, 10);
  eq(typeof EventEmitter.captureRejectionSymbol, "symbol");
  eq(EventEmitter.captureRejectionSymbol, Symbol.for("nodejs.rejection"));
  eq(typeof errorMonitor, "symbol");
});
test("on/emit/off ordering and args", () => {
  const e = new EventEmitter();
  const log = [];
  e.on("x", (a, b) => log.push(["1", a, b]));
  e.prependListener("x", () => log.push("0"));
  e.once("x", () => log.push("once"));
  eq(e.emit("x", 1, 2), true);
  eq(e.emit("x", 3, 4), true);
  same(log, ["0", ["1", 1, 2], "once", "0", ["1", 3, 4]]);
  eq(e.emit("none"), false);
  eq(e.listenerCount("x"), 2);
  same(e.eventNames(), ["x"]);
});
test("removeListener and removeAllListeners with meta events", () => {
  const e = new EventEmitter();
  const meta = [];
  e.on("newListener", (t) => meta.push("new:" + String(t)));
  e.on("removeListener", (t) => meta.push("rm:" + String(t)));
  const f = () => {};
  e.on("a", f);
  e.once("a", f);
  eq(e.listeners("a").length, 2);
  eq(e.rawListeners("a")[1] !== f, true);
  e.off("a", f);
  eq(e.listenerCount("a"), 1);
  e.removeAllListeners("a");
  eq(e.listenerCount("a"), 0);
  e.removeAllListeners();
  eq(e._eventsCount, 0);
  eq(meta.includes("new:a"), true);
  eq(meta.includes("rm:a"), true);
});
test("error events", () => {
  const e = new EventEmitter();
  const boom = new Error("boom");
  eq(((() => { try { e.emit("error", boom); } catch (x) { return x; } })()), boom);
  const err = code(() => e.emit("error", "str"), "ERR_UNHANDLED_ERROR", Error);
  eq(err.context, "str");
  eq(err.message, "Unhandled error. ('str')");
  const monitored = [];
  e.on(errorMonitor, (x) => monitored.push(x));
  e.on("error", () => {});
  e.emit("error", boom);
  same(monitored.length, 1);
  eq(monitored[0], boom);
});
test("max listeners warning", () => {
  const warnings = [];
  const saved = globalThis.process;
  globalThis.process = { emitWarning: (w) => warnings.push(w) };
  try {
    const e = new EventEmitter();
    e.setMaxListeners(1);
    e.on("x", () => {});
    e.on("x", () => {});
    e.on("x", () => {});
    eq(warnings.length, 1);
    eq(warnings[0].name, "MaxListenersExceededWarning");
    eq(warnings[0].count, 2);
    eq(e.getMaxListeners(), 1);
  } finally { globalThis.process = saved; }
  code(() => new EventEmitter().setMaxListeners(-1), "ERR_OUT_OF_RANGE", RangeError);
  code(() => new EventEmitter().on("x", 1), "ERR_INVALID_ARG_TYPE", TypeError);
});
test("subclassing via class and inherits", () => {
  class A extends EventEmitter {}
  const a = new A();
  let got;
  a.on("e", (v) => { got = v; });
  a.emit("e", 7);
  eq(got, 7);
  function B() { EventEmitter.call(this); }
  node("util").inherits(B, EventEmitter);
  const b = new B();
  b.on("e", () => { got = "b"; });
  b.emit("e");
  eq(got, "b");
});
atest("once resolves with args and rejects on error", async () => {
  const e = new EventEmitter();
  const p = once(e, "ready");
  e.emit("ready", 1, 2);
  same(await p, [1, 2]);
  const q = once(e, "ready");
  const boom = new Error("bad");
  e.emit("error", boom);
  let caught;
  await q.catch((x) => { caught = x; });
  eq(caught, boom);
});
atest("once honors AbortSignal", async () => {
  const e = new EventEmitter();
  // The bare isolate has no AbortController; a minimal signal is enough for the contract.
  const handlers = [];
  const signal = { aborted: false, addEventListener: (_, h) => handlers.push(h), removeEventListener() {} };
  const p = once(e, "never", { signal });
  signal.aborted = true;
  handlers.forEach((h) => h());
  let caught;
  await p.catch((x) => { caught = x; });
  eq(caught.code, "ABORT_ERR");
  eq(caught.name, "AbortError");
  eq(e.listenerCount("never"), 0);
});
atest("on async iterator", async () => {
  const e = new EventEmitter();
  const it = on(e, "tick");
  e.emit("tick", 1);
  e.emit("tick", 2);
  const seen = [];
  for await (const [v] of it) {
    seen.push(v);
    if (seen.length === 2) break;
  }
  same(seen, [1, 2]);
  eq(e.listenerCount("tick"), 0);
});
atest("captureRejections", async () => {
  const e = new EventEmitter({ captureRejections: true });
  const boom = new Error("async");
  const got = new Promise((res) => e.on("error", res));
  e.on("go", async () => { throw boom; });
  e.emit("go");
  eq(await got, boom);
  const f = new EventEmitter({ captureRejections: true });
  const rej = new Promise((res) => { f[EventEmitter.captureRejectionSymbol] = (err, type) => res([err, type]); });
  f.on("go", async () => { throw boom; });
  f.emit("go");
  const [err, type] = await rej;
  eq(err, boom);
  eq(type, "go");
});
test("kernel errors pass through emit untouched", () => {
  const kernel = Object.assign(new Error("denied"), { code: "VELYR_PERMISSION_DENIED" });
  const e = new EventEmitter();
  let thrown;
  try { e.emit("error", kernel); } catch (x) { thrown = x; }
  eq(thrown, kernel);
  eq(thrown.code, "VELYR_PERMISSION_DENIED");
});
test("statics", () => {
  const e = new EventEmitter();
  e.on("a", () => {});
  eq(EventEmitter.listenerCount(e, "a"), 1);
  eq(EventEmitter.getEventListeners(e, "a").length, 1);
  EventEmitter.setMaxListeners(3, e);
  eq(e.getMaxListeners(), 3);
});
finish();

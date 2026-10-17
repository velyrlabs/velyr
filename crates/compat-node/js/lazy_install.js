// Lazy install of compat-node (VELYR-223). Written for this runtime.
//
// The ~685 KB bootstrap is evaluated on the first use of anything node: `process`, `Buffer`,
// `setImmediate`, the node timers, `globalThis[Symbol.for("velyr.node")]` (what every `node:x`
// ESM facade reads) or the first CommonJS `require` (compat-commonjs asks `builtins.has`/`get`).
// Each global is an accessor until then and turns into the data property the eager install
// defines, with the same enumerable/writable/configurable flags and the same key order.
//
// The host ops are taken off the global now, as the eager bootstrap does, and put back only for
// the moment the bootstrap runs (it captures and deletes them again).
//
// shortcut: until the first use these globals report as accessors to getOwnPropertyDescriptor;
// they become data properties on first read. Upgrade if a caller inspects descriptors first.
//
// Returns `pick(0)` (the nextTick drain for the loop's tick hook) and `pick(1)` (the setImmediate
// dispatcher); both are no-ops/forwarders until the bootstrap ran.
(function (global, opNames) {
  "use strict";
  const NODE = Symbol.for("velyr.node");
  const bootstrapSource = global.op_node_bootstrap_source;
  const evaluate = global.eval;
  delete global.op_node_bootstrap_source;

  const saved = [];
  for (const name of opNames) {
    if (name in global) {
      saved.push([name, global[name]]);
      delete global[name];
    }
  }

  const commonjs = global[Symbol.for("velyr.commonjs")];
  const builtins = commonjs && commonjs.builtins;
  const real = { tick: undefined, dispatch: undefined };
  const getters = new Map();
  let state = "idle"; // idle -> booting -> done

  function defineIfOurs(name, value, enumerable) {
    const current = Object.getOwnPropertyDescriptor(global, name);
    // A script that assigned or deleted the global meanwhile keeps its choice.
    if (!current || current.get !== getters.get(name)) return;
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable });
  }

  function boot() {
    if (state !== "idle") return;
    state = "booting";
    for (const [name, op] of saved) global[name] = op;
    if (builtins) {
      delete builtins.has;
      delete builtins.get;
    }
    evaluate(bootstrapSource());
    // The eager bootstrap leaves this one non-configurable.
    Object.defineProperty(global, NODE, { configurable: false });
    const node = global[NODE];
    const buffer = node.require("buffer");
    const process = node.require("process");
    const timers = node.require("timers");
    defineIfOurs("Buffer", buffer.Buffer, false);
    defineIfOurs("process", process, false);
    for (const name of ["setTimeout", "clearTimeout", "setInterval", "clearInterval"]) {
      defineIfOurs(name, timers[name], true);
    }
    defineIfOurs("setImmediate", timers.setImmediate, true);
    defineIfOurs("clearImmediate", timers.clearImmediate, true);
    real.tick = process._tickCallback;
    real.dispatch = timers[Symbol.for("velyr.immediate.dispatch")];
    state = "done";
  }

  // While the bootstrap runs, a global reads as it did before the eager install: the web timers
  // were there, everything else was not.
  function lazy(name, enumerable) {
    const before = Object.getOwnPropertyDescriptor(global, name);
    const get = () => {
      if (state === "booting") return before && before.value;
      boot();
      return global[name];
    };
    const set = (value) => {
      Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable });
    };
    getters.set(name, get);
    Object.defineProperty(global, name, { get, set, enumerable, configurable: true });
  }

  lazy("Buffer", false);
  lazy("process", false);
  Object.defineProperty(global, "global", { value: global, writable: true, configurable: true, enumerable: true });
  for (const name of ["setTimeout", "clearTimeout", "setInterval", "clearInterval"]) lazy(name, true);
  lazy("setImmediate", true);
  lazy("clearImmediate", true);

  Object.defineProperty(global, NODE, {
    get() {
      if (state === "booting") return undefined;
      boot();
      return global[NODE];
    },
    configurable: true,
  });

  if (builtins) {
    for (const method of ["has", "get"]) {
      Object.defineProperty(builtins, method, {
        configurable: true,
        writable: true,
        value(...args) {
          boot();
          return Reflect.apply(Map.prototype[method], this, args);
        },
      });
    }
  }

  const drain = () => (real.tick === undefined ? false : Reflect.apply(real.tick, undefined, []));
  const dispatch = (...args) => Reflect.apply(real.dispatch, undefined, args);
  return (which) => (which === 0 ? drain : dispatch);
})(globalThis, /*OPS*/);

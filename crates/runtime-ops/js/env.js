// Velyr.env on top of the private env ops. Runs once per isolate, after bootstrap.js.
(function (global) {
  "use strict";

  const { op_env_get: getOp, op_env_has: hasOp, op_env_set: setOp, op_env_entries: entriesOp } =
    global;
  delete global.op_env_get;
  delete global.op_env_has;
  delete global.op_env_set;
  delete global.op_env_entries;

  const env = {
    get: (name) => getOp(name),
    has: (name) => hasOp(name),
    set: (name, value) => {
      setOp(name, value);
    },
    toObject: () => {
      const flat = entriesOp();
      const pairs = [];
      for (let i = 0; i < flat.length; i += 2) pairs.push([flat[i], flat[i + 1]]);
      return Object.fromEntries(pairs);
    },
  };
  Object.defineProperty(global.Velyr, "env", {
    value: Object.freeze(env),
    writable: false,
    configurable: true,
    enumerable: true,
  });
})(globalThis);

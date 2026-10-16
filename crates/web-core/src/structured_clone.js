// Bootstrap: structuredClone on top of the private op_structured_clone binding.
(function (global) {
  "use strict";

  // Capture the raw op, then remove it so user code never sees it (ADR-010).
  const cloneOp = global.op_structured_clone;
  delete global.op_structured_clone;
  const arrayFrom = Array.from;

  function transferList(options) {
    if (options === undefined || options === null) return [];
    if (typeof options !== "object" && typeof options !== "function") {
      throw new TypeError("structuredClone options must be an object");
    }
    const { transfer } = options;
    if (transfer === undefined) return [];
    if (transfer === null || typeof transfer[Symbol.iterator] !== "function") {
      throw new TypeError("structuredClone transfer must be iterable");
    }
    return arrayFrom(transfer);
  }

  // Length 1 as in the HTML spec: `options` is read from `arguments`.
  function structuredClone(value) {
    if (arguments.length === 0) {
      throw new TypeError("structuredClone requires at least 1 argument");
    }
    const transfer = transferList(arguments[1]);
    try {
      return cloneOp(value, transfer);
    } catch (error) {
      // The engine throws a plain Error named DataCloneError; surface it as a DOMException when available.
      if (error?.name === "DataCloneError" && typeof global.DOMException === "function") {
        throw new global.DOMException(error.message, "DataCloneError");
      }
      throw error;
    }
  }

  Object.defineProperty(global, "structuredClone", {
    value: structuredClone,
    writable: true,
    configurable: true,
    enumerable: true,
  });
})(globalThis);

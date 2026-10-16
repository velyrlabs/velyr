// Bootstrap: console on top of the private op binding. Runs once per isolate, before user code.
(function (global) {
  "use strict";

  // Capture the raw op, then remove it so user code never sees it (ADR-010).
  const writeOp = global.op_console_write;
  delete global.op_console_write;

  const MAX_DEPTH = 2;

  function quote(text) {
    return "'" + text.replace(/\\/g, "\\\\").replace(/'/g, "\\'").replace(/\n/g, "\\n") + "'";
  }

  function inspect(value, level, seen) {
    switch (typeof value) {
      case "string":
        return quote(value);
      case "number":
        return Object.is(value, -0) ? "-0" : String(value);
      case "bigint":
        return value + "n";
      case "symbol":
        return value.toString();
      case "function":
        return value.name ? "[Function: " + value.name + "]" : "[Function (anonymous)]";
      case "object":
        break;
      default:
        return String(value);
    }
    if (value === null) return "null";
    if (value instanceof Error) return String(value.stack || value);
    if (seen.includes(value)) return "[Circular]";
    const isArray = Array.isArray(value);
    if (level > MAX_DEPTH) return isArray ? "[Array]" : "[Object]";
    seen.push(value);
    try {
      const parts = isArray
        ? Array.from(value, (item) => inspect(item, level + 1, seen))
        : Object.keys(value).map((key) => key + ": " + inspect(value[key], level + 1, seen));
      const [open, close] = isArray ? ["[", "]"] : ["{", "}"];
      return parts.length ? open + " " + parts.join(", ") + " " + close : open + close;
    } finally {
      seen.pop();
    }
  }

  function display(value) {
    return typeof value === "string" ? value : inspect(value, 0, []);
  }

  function formatSpecifier(kind, arg) {
    switch (kind) {
      case "s":
        return typeof arg === "bigint" ? arg + "n" : display(arg);
      case "d":
        if (typeof arg === "bigint") return arg + "n";
        return typeof arg === "symbol" ? "NaN" : String(Number(arg));
      default:
        return inspect(arg, 0, []);
    }
  }

  function format(args) {
    const [first, ...rest] = args;
    if (typeof first !== "string") return args.map(display).join(" ");
    let used = 0;
    const head = first.replace(/%([sdoO%])/g, (match, kind) => {
      if (kind === "%") return "%";
      if (used >= rest.length) return match;
      return formatSpecifier(kind, rest[used++]);
    });
    return [head, ...rest.slice(used).map(display)].join(" ");
  }

  const console = {};
  for (const level of ["log", "info", "debug", "warn", "error"]) {
    Object.defineProperty(console, level, {
      value: function (...args) {
        writeOp(level, format(args));
      },
      writable: true,
      configurable: true,
      enumerable: true,
    });
  }
  Object.defineProperty(global, "console", {
    value: console,
    writable: true,
    configurable: true,
    enumerable: false,
  });

  // Shared `Velyr` namespace. Other bootstrap files add their members with
  // Object.defineProperty(globalThis.Velyr, ...); none of them may replace the object.
  if (!Object.hasOwn(global, "Velyr")) {
    Object.defineProperty(global, "Velyr", {
      value: {},
      writable: true,
      configurable: true,
      enumerable: false,
    });
  }
})(globalThis);

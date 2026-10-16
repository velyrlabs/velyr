// CommonJS loader (Node 24 semantics) on top of the private op_cjs_* ops. Evaluated once per
// isolate. Written for this runtime; no Node source is copied.
(function (global) {
  "use strict";

  const resolveOp = global.op_cjs_resolve;
  const readOp = global.op_cjs_read;
  const requireEsmOp = global.op_cjs_require_esm;
  delete global.op_cjs_resolve;
  delete global.op_cjs_read;
  delete global.op_cjs_require_esm;

  const cache = Object.create(null);
  // Name -> exports for host-provided modules (`node:fs` and friends); empty until one registers.
  const builtins = new Map();
  // Builtins that exist only as `node:<name>` (a bare name is an npm package), such as `test`.
  const prefixOnly = new Set();

  const failure = (Class, code, message) => {
    const error = new Class(message);
    error.code = code;
    return error;
  };

  const dirname = (path) => {
    const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    return cut <= 0 ? path.slice(0, cut + 1) || "." : path.slice(0, cut);
  };

  function nodeModulesPaths(dir) {
    const paths = [];
    for (let current = dir; ; current = dirname(current)) {
      if (!current.endsWith("/node_modules")) paths.push(`${current === "/" ? "" : current}/node_modules`);
      if (current === "/" || current === dirname(current)) break;
    }
    return paths;
  }

  class Module {
    constructor(id, parent) {
      this.id = id;
      this.path = dirname(id);
      this.exports = {};
      this.filename = id;
      this.loaded = false;
      this.children = [];
      this.paths = nodeModulesPaths(this.path);
      this.parent = parent;
    }
  }

  function requireStack(parent) {
    const stack = [];
    for (let m = parent; m; m = m.parent) stack.push(m.filename);
    return stack;
  }

  function resolveFrom(request, parent) {
    if (typeof request !== "string") {
      throw failure(TypeError, "ERR_INVALID_ARG_TYPE", `The "id" argument must be of type string. Received ${typeof request}`);
    }
    if (request === "") {
      throw failure(TypeError, "ERR_INVALID_ARG_VALUE", "The argument 'id' must be a non-empty string.");
    }
    const bare = request.startsWith("node:") ? request.slice(5) : request;
    if (builtins.has(bare) && !(bare === request && prefixOnly.has(bare))) return { builtin: bare };
    try {
      return JSON.parse(resolveOp(request, parent.filename));
    } catch (error) {
      if (!error || error.code !== "VELYR_NOT_FOUND") throw error;
      const stack = requireStack(parent);
      const lines = stack.map((file) => `\n- ${file}`).join("");
      const missing = failure(Error, "MODULE_NOT_FOUND", `Cannot find module '${request}'\nRequire stack:${lines}`);
      missing.requireStack = stack;
      throw missing;
    }
  }

  function importFrom(filename, specifier) {
    const spec = String(specifier);
    if (spec.startsWith("./") || spec.startsWith("../") || spec.startsWith("/")) {
      return import(spec.startsWith("/") ? spec : `${dirname(filename)}/${spec}`);
    }
    if (spec.startsWith("node:") || spec.startsWith("file:") || spec.startsWith("data:")) return import(spec);
    try {
      return import(JSON.parse(resolveOp(spec, filename)).path);
    } catch (error) {
      return Promise.reject(error);
    }
  }

  function run(module, format) {
    const text = readOp(module.filename);
    if (format === "json") {
      try {
        module.exports = JSON.parse(text.charCodeAt(0) === 0xfeff ? text.slice(1) : text);
      } catch (error) {
        error.message = `${module.filename}: ${error.message}`;
        throw error;
      }
      return;
    }
    const body = text.startsWith("#!") ? `//${text}` : text;
    const factory = new Function(
      "__velyrImport",
      `return function (exports, require, module, __filename, __dirname) {${body}\n}\n//# sourceURL=${module.filename}`,
    );
    const wrapper = factory((specifier) => importFrom(module.filename, specifier));
    wrapper.call(module.exports, module.exports, makeRequire(module), module, module.filename, module.path);
  }

  function load(filename, parent, format) {
    let module = cache[filename];
    if (module) {
      if (parent && !parent.children.includes(module)) parent.children.push(module);
      return module.exports;
    }
    module = new Module(filename, parent);
    cache[filename] = module;
    if (parent) parent.children.push(module);
    let done = false;
    try {
      run(module, format);
      done = true;
    } finally {
      if (!done) {
        delete cache[filename];
        if (parent) parent.children.splice(parent.children.indexOf(module), 1);
      }
    }
    module.loaded = true;
    return module.exports;
  }

  function makeRequire(module) {
    function require(id) {
      const found = resolveFrom(id, module);
      if (found.builtin !== undefined) return builtins.get(found.builtin);
      if (found.format === "esm") {
        const namespace = requireEsmOp(found.path);
        if (namespace === undefined) {
          throw failure(
            Error,
            "ERR_REQUIRE_ASYNC_MODULE",
            `require() cannot be used on an ESM graph with top-level await: ${found.path} (from ${module.filename}); use import()`,
          );
        }
        return namespace;
      }
      return load(found.path, module, found.format);
    }
    require.resolve = (request) => {
      const found = resolveFrom(request, module);
      return found.builtin !== undefined ? request : found.path;
    };
    require.cache = cache;
    module.require = require;
    return require;
  }

  const hook = Object.freeze({
    // Used by the ESM facade: runs (or reuses) a CommonJS or JSON file and returns module.exports.
    importCjs: (filename, format) => load(filename, undefined, format),
    // `require` for code that is not CommonJS (ESM, tests): resolves relative to `filename`.
    createRequire: (filename) => makeRequire(new Module(filename, undefined)),
    builtins,
    prefixOnly,
  });
  Object.defineProperty(global, Symbol.for("velyr.commonjs"), { value: hook });
})(globalThis);

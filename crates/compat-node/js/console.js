'use strict';
// node:console: the global console plus a `Console` class over any pair of writable streams
// (undici's mock formatter builds one). Written for this runtime; no Node source is copied.
//
// Known limits: no `table`, `time*` or `trace` on a custom Console (table throws
// ERR_VELYR_UNSUPPORTED); `inspectOptions` only applies to `dir`.
const errors = require('internal/errors');
const util = require('util');

const unsupported = (what) => errors.make(Error, 'ERR_VELYR_UNSUPPORTED', `console.${what} is not supported by this runtime`);

function validateStream(stream, name) {
  if (stream === null || typeof stream !== 'object' || typeof stream.write !== 'function') {
    throw errors.invalidArgType(name, 'stream.Writable', stream);
  }
}

class Console {
  #out;
  #err;
  #inspectOptions;
  #indent = '';
  #counts = new Map();

  constructor(options, stderr) {
    if (options !== null && typeof options === 'object' && typeof options.write === 'function') {
      options = { stdout: options, stderr };
    }
    errors.validateObject(options, 'options');
    validateStream(options.stdout, 'options.stdout');
    if (options.stderr !== undefined) validateStream(options.stderr, 'options.stderr');
    this.#out = options.stdout;
    this.#err = options.stderr === undefined ? options.stdout : options.stderr;
    this.#inspectOptions = options.inspectOptions;
    // Like Node: methods keep working when detached (`const { log } = console`).
    for (const name of Object.getOwnPropertyNames(Console.prototype)) {
      if (name !== 'constructor') this[name] = this[name].bind(this);
    }
  }

  #write(stream, args) {
    const text = util.format(...args);
    stream.write((this.#indent ? text.replace(/^/gm, this.#indent) : text) + '\n');
  }

  log(...args) { this.#write(this.#out, args); }
  info(...args) { this.#write(this.#out, args); }
  debug(...args) { this.#write(this.#out, args); }
  error(...args) { this.#write(this.#err, args); }
  warn(...args) { this.#write(this.#err, args); }

  dir(object, options) {
    this.#write(this.#out, [util.inspect(object, { customInspect: false, ...this.#inspectOptions, ...options })]);
  }

  assert(condition, ...args) {
    if (condition) return;
    this.warn(...(args.length === 0 ? ['Assertion failed'] : [`Assertion failed: ${args[0]}`, ...args.slice(1)]));
  }

  group(...label) {
    if (label.length > 0) this.log(...label);
    this.#indent += '  ';
  }

  groupCollapsed(...label) { this.group(...label); }
  groupEnd() { this.#indent = this.#indent.slice(0, -2); }

  count(label = 'default') {
    label = `${label}`;
    const next = (this.#counts.get(label) || 0) + 1;
    this.#counts.set(label, next);
    this.log(`${label}: ${next}`);
  }

  countReset(label = 'default') { this.#counts.delete(`${label}`); }
  table() { throw unsupported('table'); }
}

module.exports = globalThis.console;
module.exports.Console = Console;

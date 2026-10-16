'use strict';
// node:util. inspect/format live in internal/inspect, deep equality in internal/comparisons.
const errors = require('internal/errors');
const types = require('internal/types');
const { inspect } = require('internal/inspect');
const { isDeepStrictEqual } = require('internal/comparisons');
const { parseArgs } = require('internal/parse_args');

const ansiPattern = /[\u001B\u009B][[\]()#;?]*(?:(?:(?:(?:;[-a-zA-Z\d/#&.:=?%@~_]+)*|[a-zA-Z\d]+(?:;[-a-zA-Z\d/#&.:=?%@~_]*)*)?\u0007)|(?:(?:\d{1,4}(?:;\d{0,4})*)?[\dA-PR-TZcf-ntqry=><~]))/g;

function formatWithOptions(inspectOptions, ...args) {
  if (inspectOptions === null || typeof inspectOptions !== 'object') throw errors.invalidArgType('inspectOptions', 'object', inspectOptions);
  const first = args[0];
  let index = 0;
  let out = '';
  let last = 0;
  if (typeof first === 'string') {
    if (args.length === 1) return first;
    for (let i = 0; i < first.length - 1; i++) {
      if (first.charCodeAt(i) !== 37) continue;
      const next = first.charCodeAt(++i);
      if (index + 1 !== args.length) {
        let piece;
        switch (next) {
          case 115: { // %s
            const v = args[++index];
            if (typeof v === 'number') piece = Object.is(v, -0) ? '-0' : `${v}`;
            else if (typeof v === 'bigint') piece = `${v}n`;
            else if (typeof v !== 'object' || v === null) piece = String(v);
            else if (typeof v.toString === 'function' && v.toString !== Object.prototype.toString && v.toString !== Error.prototype.toString && !(v instanceof Date && false)) piece = String(v);
            else piece = inspect(v, { ...inspectOptions, depth: 0, colors: false, compact: 3 });
            break;
          }
          case 106: // %j
            try { piece = JSON.stringify(args[++index]); } catch (e) {
              if (e && /circular/i.test(e.message)) piece = '[Circular]'; else throw e;
            }
            break;
          case 100: { // %d
            const v = args[++index];
            piece = typeof v === 'bigint' ? `${v}n` : typeof v === 'symbol' ? 'NaN' : formatNum(Number(v));
            break;
          }
          case 79: piece = inspect(args[++index], inspectOptions); break; // %O
          case 111: piece = inspect(args[++index], { ...inspectOptions, showHidden: true, showProxy: true, depth: 4 }); break; // %o
          case 105: { // %i
            const v = args[++index];
            piece = typeof v === 'bigint' ? `${v}n` : typeof v === 'symbol' ? 'NaN' : formatNum(parseInt(v, 10));
            break;
          }
          case 102: { // %f
            const v = args[++index];
            piece = typeof v === 'symbol' ? 'NaN' : formatNum(parseFloat(v));
            break;
          }
          case 99: index++; piece = ''; break; // %c
          case 37: out += first.slice(last, i); last = i + 1; continue;
          default: continue;
        }
        if (last !== i - 1) out += first.slice(last, i - 1);
        out += piece;
        last = i + 1;
      } else if (next === 37) {
        out += first.slice(last, i);
        last = i + 1;
      }
    }
    if (last !== 0) {
      index++;
      if (last < first.length) out += first.slice(last);
    }
  }
  while (index < args.length) {
    const value = args[index];
    out += index !== 0 ? ' ' : '';
    out += typeof value === 'string' ? value : inspect(value, inspectOptions);
    index++;
  }
  return out;
}
const formatNum = (n) => (Object.is(n, -0) ? '-0' : `${n}`);
const format = (...args) => formatWithOptions({}, ...args);

const kCustomPromisify = Symbol.for('nodejs.util.promisify.custom');
function promisify(original) {
  errors.validateFunction(original, 'original');
  if (original[kCustomPromisify]) {
    const custom = original[kCustomPromisify];
    errors.validateFunction(custom, 'util.promisify.custom');
    return Object.defineProperty(custom, kCustomPromisify, { value: custom, enumerable: false, writable: false, configurable: true });
  }
  function fn(...args) {
    return new Promise((resolve, reject) => {
      Reflect.apply(original, this, [...args, (err, ...values) => (err ? reject(err) : resolve(values[0]))]);
    });
  }
  Object.setPrototypeOf(fn, Object.getPrototypeOf(original));
  Object.defineProperty(fn, kCustomPromisify, { value: fn, enumerable: false, writable: false, configurable: true });
  return Object.defineProperties(fn, Object.getOwnPropertyDescriptors(original));
}
promisify.custom = kCustomPromisify;

function callbackify(original) {
  errors.validateFunction(original, 'original');
  function callbackified(...args) {
    const callback = args.pop();
    errors.validateFunction(callback, 'last argument');
    Reflect.apply(original, this, args).then(
      (value) => Promise.resolve().then(() => callback(null, value)),
      (reason) => {
        if (!reason) {
          const wrapped = errors.make(Error, 'ERR_FALSY_VALUE_REJECTION', 'Promise was rejected with a falsy value');
          wrapped.reason = reason;
          reason = wrapped;
        }
        Promise.resolve().then(() => callback(reason));
      },
    );
  }
  Object.defineProperties(callbackified, Object.getOwnPropertyDescriptors(original));
  Object.defineProperty(callbackified, 'name', { value: `${original.name}Callbackified` });
  return callbackified;
}

function inherits(ctor, superCtor) {
  if (typeof ctor !== 'function') throw errors.invalidArgType('ctor', 'Function', ctor);
  if (typeof superCtor !== 'function') throw errors.invalidArgType('superCtor', 'Function', superCtor);
  if (superCtor.prototype === undefined) throw errors.invalidArgType('superCtor.prototype', 'Object', superCtor.prototype);
  Object.defineProperty(ctor, 'super_', { value: superCtor, writable: true, configurable: true });
  Object.setPrototypeOf(ctor.prototype, superCtor.prototype);
}

const warned = new Set();
function deprecate(fn, message, code) {
  errors.validateFunction(fn, 'fn');
  function deprecated(...args) {
    if (!warned.has(code ?? message)) {
      if (code !== undefined) warned.add(code); else warned.add(message);
      const process = globalThis.process;
      if (process && typeof process.emitWarning === 'function') process.emitWarning(message, 'DeprecationWarning', code);
      else if (globalThis.console) globalThis.console.error(`${code ? `(${code}) ` : ''}DeprecationWarning: ${message}`);
    }
    return new.target ? Reflect.construct(fn, args, new.target) : Reflect.apply(fn, this, args);
  }
  Object.setPrototypeOf(deprecated, fn);
  if (fn.prototype) deprecated.prototype = fn.prototype;
  return deprecated;
}

const formats = require('internal/inspect').inspect.colors;
function styleText(format, text, { validateStream = true, stream } = {}) {
  errors.validateString(text, 'text');
  const list = Array.isArray(format) ? format : [format];
  let left = '';
  let right = '';
  for (const key of list) {
    const code = formats[key];
    if (code === undefined) throw errors.invalidArgValue('format', key, `must be one of: ${Object.keys(formats).map((k) => `'${k}'`).join(', ')}`);
    left += `\u001b[${code[0]}m`;
    right = `\u001b[${code[1]}m${right}`;
  }
  // shortcut: no TTY detection here, so validateStream (default) means plain text; upgrade once streams exist.
  if (validateStream && !(stream && stream.isTTY)) return text;
  return `${left}${text}${right}`;
}

function debuglog(set) {
  const enabled = new RegExp(`(^|,)\\s*${String(set).replace(/[|\\{}()[\]^$+?.]/g, '\\$&').replace(/\*/g, '.*')}\\s*(,|$)`, 'i');
  const env = globalThis.process && globalThis.process.env && globalThis.process.env.NODE_DEBUG;
  const on = Boolean(env) && enabled.test(env);
  const logger = (...args) => {
    if (on) globalThis.console.error(`${String(set).toUpperCase()}: ${format(...args)}`);
  };
  Object.defineProperty(logger, 'enabled', { get: () => on });
  return logger;
}

module.exports = {
  TextDecoder: globalThis.TextDecoder,
  TextEncoder: globalThis.TextEncoder,
  callbackify,
  debug: debuglog,
  debuglog,
  deprecate,
  format,
  formatWithOptions,
  inherits,
  inspect,
  isArray: Array.isArray,
  isDeepStrictEqual,
  parseArgs,
  promisify,
  stripVTControlCharacters(text) {
    errors.validateString(text, 'str');
    return text.replace(ansiPattern, '');
  },
  styleText,
  toUSVString: (value) => String(value).toWellFormed(),
  types,
};

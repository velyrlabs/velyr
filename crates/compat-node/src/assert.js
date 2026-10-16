'use strict';
// node:assert. Messages follow Node's wording; the diff for deep comparisons is simplified to the
// inspected expected/actual values (no line-by-line diff).
const errors = require('internal/errors');
const { inspect } = require('internal/inspect');
const { isDeepEqual, isDeepStrictEqual } = require('internal/comparisons');

const kReadable = { depth: 1000, compact: false, sorted: true, breakLength: Infinity, customInspect: false };
const show = (value) => inspect(value, kReadable);

class AssertionError extends Error {
  constructor(options) {
    if (options === null || typeof options !== 'object') throw errors.invalidArgType('options', 'Object', options);
    const { message, operator, actual, expected } = options;
    super(message !== undefined && message !== null ? String(message) : defaultMessage(operator, actual, expected));
    this.generatedMessage = message === undefined || message === null;
    Object.defineProperty(this, 'name', { value: 'AssertionError', enumerable: false, writable: true, configurable: true });
    this.code = 'ERR_ASSERTION';
    this.actual = actual;
    this.expected = expected;
    this.operator = operator;
    if (Error.captureStackTrace) Error.captureStackTrace(this, options.stackStartFn || options.stackStartFunction || AssertionError);
    // Make the stack read "AssertionError [ERR_ASSERTION]: ..." like Node.
    if (typeof this.stack === 'string') this.stack = this.stack.replace(/^AssertionError/, 'AssertionError [ERR_ASSERTION]');
  }

  toString() {
    return `${this.name} [${this.code}]: ${this.message}`;
  }
}

function defaultMessage(operator, actual, expected) {
  switch (operator) {
    case 'strictEqual': {
      const a = show(actual);
      const b = show(expected);
      return typeof actual === 'object' && actual !== null && typeof expected === 'object' && expected !== null && a === b
        ? `Values have same structure but are not reference-equal:\n\n${a}\n`
        : `Expected values to be strictly equal:\n${a.includes('\n') || b.includes('\n') ? `\n${a}\n\nshould equal\n\n${b}` : `\n${a} !== ${b}\n`}`;
    }
    case 'deepStrictEqual':
      return `Expected values to be strictly deep-equal:\n${show(actual)}\n\nshould equal\n\n${show(expected)}`;
    case 'notStrictEqual': return `Expected "actual" to be strictly unequal to: ${show(expected)}`;
    case 'notDeepStrictEqual': return `Expected "actual" not to be strictly deep-equal to: ${show(expected)}`;
    case 'deepEqual': return `Expected values to be loosely deep-equal:\n\n${show(actual)}\n\nshould loosely deep-equal\n\n${show(expected)}`;
    case 'notDeepEqual': return `Expected "actual" not to be loosely deep-equal to: ${show(expected)}`;
    case '==': case '!=': return `${inspect(actual)} ${operator} ${inspect(expected)}`;
    case 'fail': return 'Failed';
    default: return `${inspect(actual)} ${operator} ${inspect(expected)}`;
  }
}

function innerFail(obj) {
  if (obj.message instanceof Error) throw obj.message;
  throw new AssertionError(obj);
}

function innerOk(fn, argLen, value, message) {
  if (value) return;
  let generated = false;
  if (argLen === 0) {
    generated = true;
    message = 'No value argument passed to `assert.ok()`';
  } else if (message === undefined) {
    generated = true;
    message = 'The expression evaluated to a falsy value:\n\n  assert' + (fn === ok ? '.ok' : '') + `(${inspect(value)})\n`;
  } else if (message instanceof Error) {
    throw message;
  }
  const err = new AssertionError({ actual: value, expected: true, message, operator: '==', stackStartFn: fn });
  err.generatedMessage = generated;
  throw err;
}

function ok(...args) { innerOk(ok, args.length, ...args); }
function assert(...args) { innerOk(assert, args.length, ...args); }

function requireArgs(count, args, ...names) {
  if (args.length < count) throw errors.missingArgs(...names);
}
const compare = (op, test, name) => {
  const fn = function (actual, expected, message) {
    requireArgs(2, arguments, 'actual', 'expected');
    if (!test(actual, expected)) innerFail({ actual, expected, message, operator: op, stackStartFn: fn });
  };
  Object.defineProperty(fn, 'name', { value: name });
  return fn;
};
const bothNaN = (a, b) => a !== a && b !== b; // eslint-disable-line no-self-compare

assert.ok = ok;
assert.AssertionError = AssertionError;
assert.equal = compare('==', (a, b) => a == b || bothNaN(a, b), 'equal'); // eslint-disable-line eqeqeq
assert.notEqual = compare('!=', (a, b) => !(a == b || bothNaN(a, b)), 'notEqual'); // eslint-disable-line eqeqeq
assert.strictEqual = compare('strictEqual', Object.is, 'strictEqual');
assert.notStrictEqual = compare('notStrictEqual', (a, b) => !Object.is(a, b), 'notStrictEqual');
assert.deepEqual = compare('deepEqual', isDeepEqual, 'deepEqual');
assert.notDeepEqual = compare('notDeepEqual', (a, b) => !isDeepEqual(a, b), 'notDeepEqual');
assert.deepStrictEqual = compare('deepStrictEqual', isDeepStrictEqual, 'deepStrictEqual');
assert.notDeepStrictEqual = compare('notDeepStrictEqual', (a, b) => !isDeepStrictEqual(a, b), 'notDeepStrictEqual');
assert.fail = function fail(message) {
  if (message instanceof Error) throw message;
  const err = new AssertionError({ message: message === undefined ? 'Failed' : message, operator: 'fail', stackStartFn: fail });
  err.generatedMessage = message === undefined;
  throw err;
};

assert.ifError = function ifError(err) {
  if (err === null || err === undefined) return;
  let message = 'ifError got unwanted exception: ';
  message += typeof err === 'object' && typeof err.message === 'string' ? (err.message.length === 0 && err.constructor ? err.constructor.name : err.message) : inspect(err);
  const error = new AssertionError({ actual: err, expected: null, operator: 'ifError', message, stackStartFn: ifError });
  error.origin = err;
  throw error;
};

function matches(actual, expected, message, fn) {
  if (typeof expected === 'string' || !(expected instanceof RegExp)) throw errors.invalidArgType('regexp', 'RegExp', expected);
  const isMatch = fn === assert.match;
  if (typeof actual !== 'string' || (expected.test(actual) !== isMatch)) {
    if (message instanceof Error) throw message;
    const generated = message === undefined;
    const text = message !== undefined ? message : typeof actual !== 'string'
      ? `The "string" argument must be of type string. Received type ${typeof actual} (${inspect(actual)})`
      : `${isMatch ? 'The input did not match the regular expression ' : 'The input was expected to not match the regular expression '}${inspect(expected)}. Input:\n\n${inspect(actual)}\n`;
    const err = new AssertionError({ actual, expected, message: text, operator: fn.name, stackStartFn: fn });
    err.generatedMessage = generated;
    throw err;
  }
}
assert.match = function match(actual, expected, message) { matches(actual, expected, message, match); };
assert.doesNotMatch = function doesNotMatch(actual, expected, message) { matches(actual, expected, message, doesNotMatch); };

function checkExpected(actual, expected, message, fn) {
  if (expected === undefined) return;
  if (typeof expected === 'function') {
    if (expected.prototype !== undefined && actual instanceof expected) return;
    if (Error.isPrototypeOf(expected)) {
      const err = new AssertionError({ actual, expected, message: message || `The error is expected to be an instance of "${expected.name}". Received "${actual && actual.constructor && actual.constructor.name}"\n\nError message:\n\n${actual && actual.message}`, operator: fn.name, stackStartFn: fn });
      err.generatedMessage = !message;
      throw err;
    }
    if (expected.call({}, actual) === true) return;
    const err = new AssertionError({ actual, expected, message: message || `The ${expected.name ? `"${expected.name}" ` : ''}validation function is expected to return "true". Received ${inspect(expected.call({}, actual))}\n\nCaught error:\n\n${actual}`, operator: fn.name, stackStartFn: fn });
    err.generatedMessage = !message;
    throw err;
  }
  if (expected instanceof RegExp) {
    const text = String(actual);
    if (expected.exec(text) !== null) return;
    const err = new AssertionError({ actual, expected, message: message || `The input did not match the regular expression ${inspect(expected)}. Input:\n\n${inspect(text)}\n`, operator: fn.name, stackStartFn: fn });
    err.generatedMessage = !message;
    throw err;
  }
  if (typeof expected !== 'object' || expected === null) throw errors.invalidArgType('expected', ['Function', 'RegExp', 'Object', 'Error'], expected);
  const keys = Object.keys(expected);
  if (expected instanceof Error) keys.push('name', 'message');
  for (const key of keys) {
    const want = expected[key];
    const got = actual === null || actual === undefined ? undefined : actual[key];
    const same = typeof got === 'string' && want instanceof RegExp ? want.test(got) : isDeepStrictEqual(got, want);
    if (!same) {
      const err = new AssertionError({ actual, expected, message: message || `Expected values to be strictly deep-equal:\n${show(got)}\n\nshould equal\n\n${show(want)}\n\nProperty: ${key}`, operator: fn.name, stackStartFn: fn });
      err.generatedMessage = !message;
      throw err;
    }
  }
}

const NO_EXCEPTION = Symbol('no exception');
function getActual(fn) {
  if (typeof fn !== 'function') throw errors.invalidArgType('fn', 'Function', fn);
  try { fn(); } catch (e) { return e; }
  return NO_EXCEPTION;
}
async function waitForActual(promiseFn) {
  let resultPromise;
  if (typeof promiseFn === 'function') {
    resultPromise = promiseFn();
    if (!resultPromise || typeof resultPromise.then !== 'function') {
      throw errors.make(TypeError, 'ERR_INVALID_RETURN_VALUE', `Expected instance of Promise to be returned from the "promiseFn" function but got ${resultPromise === null ? 'null' : typeof resultPromise === 'object' ? `an instance of ${resultPromise.constructor && resultPromise.constructor.name}` : `type ${typeof resultPromise}`}.`);
    }
  } else if (promiseFn && typeof promiseFn.then === 'function') {
    resultPromise = promiseFn;
  } else {
    throw errors.invalidArgType('promiseFn', ['Function', 'Promise'], promiseFn);
  }
  try { await resultPromise; } catch (e) { return e; }
  return NO_EXCEPTION;
}

function expectsError(fn, actual, error, message) {
  if (typeof error === 'string') {
    if (arguments.length === 4) throw errors.invalidArgType('error', ['Object', 'Error', 'Function', 'RegExp'], error);
    message = error;
    error = undefined;
  }
  if (actual === NO_EXCEPTION) {
    let details = '';
    if (error && error.name) details += ` (${error.name})`;
    details += message ? `: ${message}` : '.';
    const operation = fn.name === 'rejects' ? 'rejection' : 'exception';
    innerFail({ actual: undefined, expected: error, operator: fn.name, message: `Missing expected ${operation}${details}`, stackStartFn: fn });
  }
  checkExpected(actual, error, message, fn);
}
function expectsNoError(fn, actual, error, message) {
  if (actual === NO_EXCEPTION) return;
  if (typeof error === 'string') { message = error; error = undefined; }
  if (!error || (typeof error === 'function' && actual instanceof error) || (error instanceof RegExp && error.test(String(actual)))) {
    const details = message ? `: ${message}` : '.';
    const operation = fn.name === 'doesNotReject' ? 'rejection' : 'exception';
    innerFail({ actual, expected: error, operator: fn.name, message: `Got unwanted ${operation}${details}\nActual message: "${actual && actual.message}"`, stackStartFn: fn });
  }
  throw actual;
}

assert.throws = function throws(fn, ...args) { expectsError(throws, getActual(fn), ...args); };
assert.rejects = async function rejects(promiseFn, ...args) { expectsError(rejects, await waitForActual(promiseFn), ...args); };
assert.doesNotThrow = function doesNotThrow(fn, ...args) { expectsNoError(doesNotThrow, getActual(fn), ...args); };
assert.doesNotReject = async function doesNotReject(fn, ...args) { expectsNoError(doesNotReject, await waitForActual(fn), ...args); };

function strict(...args) { innerOk(strict, args.length, ...args); }
Object.assign(strict, assert, {
  equal: assert.strictEqual,
  notEqual: assert.notStrictEqual,
  deepEqual: assert.deepStrictEqual,
  notDeepEqual: assert.notDeepStrictEqual,
});
strict.strict = strict;
assert.strict = strict;

module.exports = assert;

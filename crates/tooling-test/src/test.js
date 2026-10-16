// velyr:test and node:test: describe/it/test, hooks, expect and mocks. Written for this runtime.
//
// Registration is synchronous; the run starts on the first macrotask after the first call, so
// tests registered after a top-level await are not seen. `velyr test` starts each file with
// `--velyr-test` in process.argv: results are then one sentinel line each, for the parent to
// parse; without it they are plain TAP-like lines. `--velyr-test-filter=<text>` keeps the tests
// whose full name ("suite > test") contains the text. Mocks are restored after every test.
const assert = require('assert');
// Not the global: every builtin is loaded at bootstrap, before `process` is installed on it.
const process = require('process');
const { inspect } = require('util');

const SENTINEL = '\u0001velyr-test ';
const FILTER_FLAG = '--velyr-test-filter=';
// Read when used: argv is configured after the builtins load.
const isMachine = () => process.argv.includes('--velyr-test');
const filterText = () => (process.argv.find((arg) => arg.startsWith(FILTER_FLAG)) || FILTER_FLAG).slice(FILTER_FLAG.length);

// Per-test limit, like node:test's default. It only catches a test that yields: one that blocks
// the isolate is killed by the parent (`velyr test`), which sees the `start` event.
const TIMEOUT_FLAG = '--velyr-test-timeout=';
const DEFAULT_TIMEOUT_MS = 5000;
function timeoutMs() {
  const arg = process.argv.find((a) => a.startsWith(TIMEOUT_FLAG));
  const ms = arg ? Number(arg.slice(TIMEOUT_FLAG.length)) : NaN;
  return ms > 0 ? ms : DEFAULT_TIMEOUT_MS;
}

function withTimeout(promise, ms) {
  let timer;
  const limit = new Promise((resolve, reject) => {
    timer = setTimeout(() => reject(new Error(`timed out after ${ms} ms`)), ms);
  });
  return Promise.race([promise, limit]).finally(() => clearTimeout(timer));
}

function makeSuite(name, parent, skip) {
  return { name, parent, skip, items: [], hooks: { before: [], after: [], beforeEach: [], afterEach: [] } };
}

const root = makeSuite('', null, false);
let current = root;
let scheduled = false;
let failures = 0;

function schedule() {
  if (scheduled) return;
  scheduled = true;
  setTimeout(run, 0);
}

function suitePath(suite) {
  const names = [];
  for (let s = suite; s && s !== root; s = s.parent) names.unshift(s.name);
  return names;
}

function test(name, a, b, forceSkip = false) {
  const fn = typeof a === 'function' ? a : b;
  const opts = typeof a === 'function' ? {} : a || {};
  const skip = forceSkip || current.skip || opts.skip === true || typeof fn !== 'function';
  current.items.push({ kind: 'test', name: String(name), fn, skip, parent: current });
  schedule();
}

function describe(name, a, b, forceSkip = false) {
  const fn = typeof a === 'function' ? a : b;
  const opts = typeof a === 'function' ? {} : a || {};
  const suite = makeSuite(String(name), current, forceSkip || current.skip || opts.skip === true);
  current.items.push({ kind: 'suite', suite });
  const saved = current;
  current = suite;
  try {
    if (typeof fn === 'function') fn();
  } finally {
    current = saved;
  }
  schedule();
}

for (const kind of ['before', 'after', 'beforeEach', 'afterEach']) {
  test[kind] = (fn) => {
    current.hooks[kind].push(fn);
    schedule();
  };
}
test.skip = (name, a, b) => test(name, a, b, true);
test.todo = test.skip;
describe.skip = (name, a, b) => describe(name, a, b, true);
describe.todo = describe.skip;

// Output: one sentinel line per event for `velyr test`, plain lines otherwise.
function out(event) {
  if (isMachine()) {
    process.stdout.write(`${SENTINEL}${JSON.stringify(event)}\n`);
  } else if (event.type === 'test') {
    const mark = event.status === 'fail' ? 'not ok' : 'ok';
    const note = event.status === 'skip' ? ' # SKIP' : '';
    process.stdout.write(`${mark} - ${event.path.join(' > ')}${note}\n`);
    if (event.error) process.stdout.write(`${event.error.split('\n').map((l) => `# ${l}`).join('\n')}\n`);
  }
}

function describeError(error) {
  if (error === null || typeof error !== 'object') return String(error);
  const stack = String(error.stack || '');
  const message = String(error.message);
  return stack.includes(message) ? stack : `${message}\n${stack}`;
}

function report(path, status, ms, error) {
  if (status === 'fail') failures++;
  const event = { type: 'test', path, status, ms };
  if (status === 'fail') event.error = describeError(error);
  out(event);
}

const pathOf = (item) => [...suitePath(item.parent), item.name];
const wants = (item) => filterText() === '' || pathOf(item).join(' > ').includes(filterText());
const hasMatch = (suite) => suite.items.some((item) => (item.kind === 'test' ? wants(item) : hasMatch(item.suite)));

async function runTest(item, chain) {
  const path = pathOf(item);
  if (item.skip) {
    report(path, 'skip', 0);
    return;
  }
  const started = Date.now();
  const context = { name: item.name, mock };
  let failed = false;
  let failure;
  out({ type: 'start', path });
  try {
    await withTimeout((async () => {
      for (const suite of chain) for (const hook of suite.hooks.beforeEach) await hook(context);
      await item.fn(context);
    })(), timeoutMs());
  } catch (error) {
    failed = true;
    failure = error;
  }
  for (const suite of [...chain].reverse()) {
    for (const hook of suite.hooks.afterEach) {
      try {
        await hook(context);
      } catch (error) {
        if (!failed) [failed, failure] = [true, error];
      }
    }
  }
  mock.restoreAll();
  report(path, failed ? 'fail' : 'pass', Date.now() - started, failure);
}

async function runSuite(suite, chain) {
  if (!hasMatch(suite)) return;
  const here = [...chain, suite];
  try {
    for (const hook of suite.hooks.before) await hook();
  } catch (error) {
    report([...suitePath(suite), 'before hook'], 'fail', 0, error);
    return;
  }
  for (const item of suite.items) {
    if (item.kind === 'suite') await runSuite(item.suite, here);
    else if (wants(item)) await runTest(item, here);
  }
  try {
    for (const hook of suite.hooks.after) await hook();
  } catch (error) {
    report([...suitePath(suite), 'after hook'], 'fail', 0, error);
  }
}

async function run() {
  try {
    await runSuite(root, []);
  } catch (error) {
    report(['(runner)'], 'fail', 0, error);
  }
  out({ type: 'end' });
  process.exit(failures > 0 ? 1 : 0);
}

// expect: toBe, toEqual, toMatchObject, toThrow, with .not, .resolves and .rejects.
const isEqual = (a, b) => {
  try {
    assert.deepStrictEqual(a, b);
    return true;
  } catch {
    return false;
  }
};

function isSubset(actual, expected) {
  if (expected === null || typeof expected !== 'object') return isEqual(actual, expected);
  if (actual === null || typeof actual !== 'object') return false;
  if (Array.isArray(expected)) {
    return Array.isArray(actual) && actual.length === expected.length && expected.every((v, i) => isSubset(actual[i], v));
  }
  if (expected instanceof Date || expected instanceof RegExp || expected instanceof Map || expected instanceof Set) {
    return isEqual(actual, expected);
  }
  return Object.keys(expected).every((key) => key in actual && isSubset(actual[key], expected[key]));
}

function thrownMatches(thrown, expected) {
  if (expected === undefined) return true;
  const message = thrown !== null && typeof thrown === 'object' ? String(thrown.message) : String(thrown);
  if (typeof expected === 'string') return message.includes(expected);
  if (expected instanceof RegExp) return expected.test(message);
  if (typeof expected === 'function') return thrown instanceof expected;
  if (expected !== null && typeof expected === 'object' && 'message' in expected) return message === String(expected.message);
  return false;
}

function verdict(name, actual, expected, thrown) {
  switch (name) {
    case 'toBe':
      return { pass: Object.is(actual, expected), subject: inspect(actual), predicate: `to be ${inspect(expected)}` };
    case 'toEqual':
      return { pass: isEqual(actual, expected), subject: inspect(actual), predicate: `to equal ${inspect(expected)}` };
    case 'toMatchObject':
      return { pass: isSubset(actual, expected), subject: inspect(actual), predicate: `to match ${inspect(expected)}` };
    default:
      return {
        pass: thrown.did && thrownMatches(thrown.value, expected),
        subject: thrown.did ? `the thrown ${inspect(thrown.value)}` : 'the function',
        predicate: expected === undefined ? 'to throw' : `to throw ${inspect(expected)}`,
      };
  }
}

function judge(name, negate, actual, args, thrown) {
  const { pass, subject, predicate } = verdict(name, actual, args[0], thrown);
  if (negate ? pass : !pass) {
    throw new assert.AssertionError({
      message: `expected ${subject} ${negate ? 'not ' : ''}${predicate}`,
      actual,
      expected: args[0],
      operator: name,
    });
  }
}

const MATCHERS = ['toBe', 'toEqual', 'toMatchObject', 'toThrow'];

function call(fn) {
  if (typeof fn !== 'function') throw new TypeError('expect(fn).toThrow: the received value must be a function');
  try {
    fn();
    return { did: false };
  } catch (value) {
    return { did: true, value };
  }
}

const syncMatchers = (actual, negate) => Object.fromEntries(MATCHERS.map((name) => [
  name,
  (...args) => judge(name, negate, actual, args, name === 'toThrow' ? call(actual) : null),
]));

const settled = (promise, mode, negate) => Object.fromEntries(MATCHERS.map((name) => [
  name,
  async (...args) => {
    let value;
    let rejected = false;
    try {
      value = await promise;
    } catch (error) {
      rejected = true;
      value = error;
    }
    if (mode === 'resolves' && rejected) {
      throw new assert.AssertionError({ message: `expected the promise to resolve, but it rejected with ${inspect(value)}`, operator: mode });
    }
    if (mode === 'rejects' && !rejected) {
      throw new assert.AssertionError({ message: `expected the promise to reject, but it resolved with ${inspect(value)}`, operator: mode });
    }
    if (name !== 'toThrow') return judge(name, negate, value, args, null);
    return judge(name, negate, value, args, mode === 'rejects' ? { did: true, value } : call(value));
  },
]));

function expect(actual) {
  const matchers = syncMatchers(actual, false);
  matchers.not = syncMatchers(actual, true);
  for (const mode of ['resolves', 'rejects']) {
    matchers[mode] = { ...settled(actual, mode, false), not: settled(actual, mode, true) };
  }
  return matchers;
}

// mock.fn and mock.method, shaped like node:test: `fn.mock.calls[i].{arguments,result,error}`.
const live = [];

function createMock(implementation, restore) {
  let implement = implementation;
  const once = [];
  const calls = [];
  const fn = function mocked(...args) {
    const record = { arguments: args, result: undefined, error: undefined, this: this };
    calls.push(record);
    try {
      record.result = (once.length > 0 ? once.shift() : implement).apply(this, args);
      return record.result;
    } catch (error) {
      record.error = error;
      throw error;
    }
  };
  fn.mock = {
    calls,
    callCount: () => calls.length,
    mockImplementation(next) { implement = next; },
    mockImplementationOnce(next) { once.push(next); },
    resetCalls() { calls.length = 0; },
    restore,
  };
  live.push(fn.mock);
  return fn;
}

const mock = {
  fn(original = () => undefined, implementation = original) {
    return createMock(implementation, () => undefined);
  },
  method(object, name, implementation) {
    const original = object[name];
    if (typeof original !== 'function') throw new TypeError(`mock.method: ${String(name)} is not a function`);
    const own = Object.getOwnPropertyDescriptor(object, name);
    const mocked = createMock(implementation || original, () => {
      if (own) Object.defineProperty(object, name, own);
      else delete object[name];
    });
    Object.defineProperty(object, name, { value: mocked, writable: true, configurable: true, enumerable: own ? own.enumerable : false });
    return mocked;
  },
  restoreAll() {
    for (const entry of live.splice(0)) entry.restore();
  },
};
mock.reset = mock.restoreAll;

test.test = test;
test.it = test;
test.describe = describe;
test.expect = expect;
test.mock = mock;
module.exports = test;

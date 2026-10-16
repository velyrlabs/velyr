// Adapted from nodejs/node test/parallel/test-util-promisify.js and test-util-types.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const util = require('node:util');

const cb = (a, done) => setImmediate(() => done(null, a * 2));
const fail = (done) => setImmediate(() => done(new Error('nope')));
assert.strictEqual(util.types.isPromise(Promise.resolve()), true);
assert.strictEqual(util.types.isRegExp(/a/), true);
assert.strictEqual(util.types.isDate(new Date()), true);
assert.strictEqual(util.types.isTypedArray(new Uint8Array(1)), true);
assert.strictEqual(util.types.isMap(new Map()), true);
assert.strictEqual(util.isDeepStrictEqual({ a: 1 }, { a: 1 }), true);
assert.strictEqual(util.isArray([]), true);

(async () => {
  assert.strictEqual(await util.promisify(cb)(21), 42);
  await assert.rejects(util.promisify(fail)(), { message: 'nope' });
  const old = util.callbackify(async (x) => x + 1);
  await new Promise((resolve, reject) => old(1, (err, v) => {
    try { assert.strictEqual(err, null); assert.strictEqual(v, 2); resolve(); } catch (e) { reject(e); }
  }));
})().catch((err) => {
  console.error(err);
  process.exit(1);
});

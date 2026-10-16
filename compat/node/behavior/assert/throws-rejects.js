// Adapted from nodejs/node test/parallel/test-assert-async.js and test-assert.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');

assert.throws(() => { throw new TypeError('bad'); }, TypeError);
assert.throws(() => { throw new Error('bad value'); }, /bad/);
assert.throws(() => { throw new Error('x'); }, { message: 'x', name: 'Error' });
assert.doesNotThrow(() => 1);
assert.throws(() => assert.throws(() => {}), { code: 'ERR_ASSERTION' });
assert.throws(() => assert.ifError(new Error('e')), { code: 'ERR_ASSERTION' });
assert.ifError(null);

(async () => {
  await assert.rejects(Promise.reject(new RangeError('r')), RangeError);
  await assert.doesNotReject(Promise.resolve(1));
  await assert.rejects(assert.rejects(Promise.resolve(1)), { code: 'ERR_ASSERTION' });
})().catch((err) => {
  console.error(err);
  process.exit(1);
});

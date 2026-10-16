// Adapted from nodejs/node test/parallel/test-assert-fail.js and test-assert-first-line.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');

assert.strictEqual(1, 1);
assert.notStrictEqual(1, 2);
assert.ok(true);
assert.equal(1, '1');
assert.notEqual(1, 2);
assert.match('abc', /b/);
assert.doesNotMatch('abc', /z/);
try {
  assert.strictEqual(1, 2, 'custom message');
  assert.fail('unreachable');
} catch (err) {
  assert.ok(err instanceof assert.AssertionError);
  assert.ok(err.message.startsWith('custom message'));
  assert.strictEqual(err.actual, 1);
  assert.strictEqual(err.expected, 2);
  assert.strictEqual(err.operator, 'strictEqual');
  assert.strictEqual(err.code, 'ERR_ASSERTION');
}
assert.throws(() => assert.fail('boom'), { message: 'boom' });
assert.strictEqual(assert.strict.equal, assert.strictEqual);

// Adapted from nodejs/node test/parallel/test-assert-deep.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');

assert.deepStrictEqual({ a: [1, { b: 2 }] }, { a: [1, { b: 2 }] });
assert.deepStrictEqual(new Map([[1, 'a']]), new Map([[1, 'a']]));
assert.deepStrictEqual(new Set([1, 2]), new Set([2, 1]));
assert.deepStrictEqual(new Date(5), new Date(5));
assert.deepStrictEqual([1, , 3].length, 3);
assert.throws(() => assert.deepStrictEqual({ a: 1 }, { a: '1' }), { code: 'ERR_ASSERTION' });
assert.throws(() => assert.deepStrictEqual([1], { 0: 1 }), { code: 'ERR_ASSERTION' });
assert.throws(() => assert.deepStrictEqual(0, -0), { code: 'ERR_ASSERTION' });
assert.notDeepStrictEqual({ a: 1 }, { a: 2 });
assert.deepEqual({ a: 1 }, { a: '1' });
assert.throws(() => assert.deepStrictEqual(new Set([1]), new Set([2])), assert.AssertionError);

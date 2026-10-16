// Adapted from nodejs/node test/parallel/test-buffer-alloc.js and test-buffer-concat.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { Buffer } = require('node:buffer');

assert.strictEqual(Buffer.alloc(3, 1).toString('hex'), '010101');
assert.strictEqual(Buffer.alloc(4, 'ab').toString(), 'abab');
assert.strictEqual(Buffer.from('héllo').length, 6);
assert.deepStrictEqual([...Buffer.from([1, 2, 300])], [1, 2, 44]);
assert.strictEqual(Buffer.concat([Buffer.from('a'), Buffer.from('bc')]).toString(), 'abc');
assert.strictEqual(Buffer.concat([Buffer.from('abc')], 2).toString(), 'ab');
assert.strictEqual(Buffer.byteLength('€'), 3);
assert.strictEqual(Buffer.isBuffer(Buffer.alloc(0)), true);
assert.strictEqual(Buffer.isBuffer(new Uint8Array(0)), false);
assert.strictEqual(Buffer.allocUnsafe(8).length, 8);
assert.throws(() => Buffer.alloc(-1), { code: 'ERR_OUT_OF_RANGE' });
assert.throws(() => Buffer.from({}), { code: 'ERR_INVALID_ARG_TYPE' });
assert.ok(Buffer.from('a') instanceof Uint8Array);

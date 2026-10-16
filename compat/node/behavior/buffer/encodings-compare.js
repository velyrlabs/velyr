// Adapted from nodejs/node test/parallel/test-buffer-tostring.js and test-buffer-compare.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { Buffer } = require('node:buffer');

const b = Buffer.from('hello world');
assert.strictEqual(b.toString('base64'), 'aGVsbG8gd29ybGQ=');
assert.strictEqual(b.toString('base64url'), 'aGVsbG8gd29ybGQ');
assert.strictEqual(b.toString('hex'), '68656c6c6f20776f726c64');
assert.strictEqual(Buffer.from('aGk=', 'base64').toString(), 'hi');
assert.strictEqual(Buffer.from('6869', 'hex').toString(), 'hi');
assert.strictEqual(Buffer.from('é', 'latin1').length, 1);
assert.strictEqual(Buffer.from('hi', 'utf16le').length, 4);
assert.strictEqual(b.toString('utf8', 0, 5), 'hello');
assert.strictEqual(Buffer.compare(Buffer.from('a'), Buffer.from('b')), -1);
assert.strictEqual(Buffer.from('abc').equals(Buffer.from('abc')), true);
assert.strictEqual(b.indexOf('world'), 6);
assert.strictEqual(b.includes('xyz'), false);
assert.strictEqual(b.subarray(6).toString(), 'world');
assert.strictEqual(b.slice(0, 1).toString(), 'h');
assert.deepStrictEqual(Buffer.from('ab').toJSON(), { type: 'Buffer', data: [97, 98] });

// Adapted from nodejs/node test/parallel/test-querystring.js (stringify section). Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const qs = require('node:querystring');

assert.strictEqual(qs.stringify({ a: 1, b: ['x', 'y'] }), 'a=1&b=x&b=y');
assert.strictEqual(qs.stringify({ a: 'b c', d: 'é' }), 'a=b%20c&d=%C3%A9');
assert.strictEqual(qs.stringify({ a: 1, b: 2 }, ';', ':'), 'a:1;b:2');
assert.strictEqual(qs.stringify({ a: undefined, b: null, c: true }), 'a=&b=&c=true');
assert.strictEqual(qs.stringify({ a: {} }), 'a=');
assert.strictEqual(qs.stringify('x'), '');
assert.strictEqual(qs.stringify({ a: 10n }), 'a=10');

// Adapted from nodejs/node test/parallel/test-querystring.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const qs = require('node:querystring');

assert.deepStrictEqual({ ...qs.parse('a=1&b=2&a=3') }, { a: ['1', '3'], b: '2' });
assert.deepStrictEqual({ ...qs.parse('a=%20b+c') }, { a: ' b c' });
assert.deepStrictEqual({ ...qs.parse('a:1;b:2', ';', ':') }, { a: '1', b: '2' });
assert.deepStrictEqual({ ...qs.parse('a=1&b=2&c=3', null, null, { maxKeys: 2 }) }, { a: '1', b: '2' });
assert.deepStrictEqual({ ...qs.parse('') }, {});
assert.deepStrictEqual({ ...qs.parse('flag') }, { flag: '' });
assert.strictEqual(Object.getPrototypeOf(qs.parse('a=1')), null);
assert.deepStrictEqual({ ...qs.parse('a=%zz') }, { a: '%zz' });

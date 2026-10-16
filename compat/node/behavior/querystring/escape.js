// Adapted from nodejs/node test/parallel/test-querystring-escape.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const qs = require('node:querystring');

assert.strictEqual(qs.escape('a b&c'), 'a%20b%26c');
assert.strictEqual(qs.escape('é'), '%C3%A9');
assert.strictEqual(qs.unescape('a%20b%26c'), 'a b&c');
assert.strictEqual(qs.unescape('%zz'), '%zz');
assert.strictEqual(qs.unescape('%E2%82%AC'), '€');
assert.strictEqual(qs.escape(5), '5');
assert.throws(() => qs.escape('\ud800'), { code: 'ERR_INVALID_URI' });
assert.strictEqual(qs.encode, qs.stringify);
assert.strictEqual(qs.decode, qs.parse);

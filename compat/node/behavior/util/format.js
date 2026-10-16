// Adapted from nodejs/node test/parallel/test-util-format.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const util = require('node:util');

assert.strictEqual(util.format('%s:%d', 'a', 5), 'a:5');
assert.strictEqual(util.format('%i', '42.9'), '42');
assert.strictEqual(util.format('%f', '1.5'), '1.5');
assert.strictEqual(util.format('%j', { a: 1 }), '{"a":1}');
assert.strictEqual(util.format('%% %s', 'x'), '% x');
assert.strictEqual(util.format('%s'), '%s');
assert.strictEqual(util.format('a', 'b', 3), 'a b 3');
assert.strictEqual(util.format('%o', 1), '1');
assert.strictEqual(util.format(1, 2), '1 2');
assert.strictEqual(util.format('%s', null), 'null');
assert.strictEqual(util.format('%d', '0x10'), '16');

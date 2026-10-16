// Adapted from nodejs/node test/parallel/test-tty-isatty.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const tty = require('node:tty');

for (const fd of [0, 1, 2]) assert.strictEqual(typeof tty.isatty(fd), 'boolean');
for (const fd of [-1, 1.5, '1', null, undefined, NaN, 2 ** 31, {}]) assert.strictEqual(tty.isatty(fd), false);
assert.strictEqual(tty.isatty(99999), false);
assert.strictEqual(typeof tty.ReadStream, 'function');
assert.strictEqual(typeof tty.WriteStream, 'function');
assert.strictEqual(tty.isatty.length, 1);
// stdout agrees with isatty on its own fd.
assert.strictEqual(Boolean(process.stdout.isTTY), process.stdout.isTTY === true);

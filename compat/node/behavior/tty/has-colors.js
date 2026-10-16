// Adapted from nodejs/node test/parallel/test-tty-get-color-depth.js (hasColors section). Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { WriteStream } = require('node:tty');

const has = (...args) => WriteStream.prototype.hasColors.apply(WriteStream.prototype, args);
assert.strictEqual(has({ FORCE_COLOR: '3' }), true);
assert.strictEqual(has({ NO_COLOR: '1' }), false);
assert.strictEqual(has(256, { FORCE_COLOR: '2' }), true);
assert.strictEqual(has(257, { FORCE_COLOR: '2' }), false);
assert.strictEqual(has(2, { NO_COLOR: '1' }), true);
assert.strictEqual(has(16777216, { COLORTERM: 'truecolor' }), true);
assert.strictEqual(has(16, { TERM: 'xterm' }), true);
assert.strictEqual(has(16, { TERM: 'dumb' }), false);

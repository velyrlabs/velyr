// Adapted from nodejs/node test/parallel/test-tty-get-color-depth.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { WriteStream } = require('node:tty');

const depth = (env) => WriteStream.prototype.getColorDepth.call(null, env);
assert.strictEqual(depth({}), 1);
assert.strictEqual(depth({ NO_COLOR: '1' }), 1);
assert.strictEqual(depth({ TERM: 'dumb' }), 1);
assert.strictEqual(depth({ FORCE_COLOR: '0' }), 1);
assert.strictEqual(depth({ FORCE_COLOR: '2' }), 8);
assert.strictEqual(depth({ FORCE_COLOR: '3' }), 24);
assert.strictEqual(depth({ FORCE_COLOR: '3', NO_COLOR: '1' }), 24);
assert.strictEqual(depth({ COLORTERM: 'truecolor' }), 24);
assert.strictEqual(depth({ TERM: 'xterm-256color' }), 8);
assert.strictEqual(depth({ TERM: 'xterm' }), 4);

// Adapted from nodejs/node test/parallel/test-os.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const os = require('node:os');

assert.strictEqual(os.EOL, '\n');
assert.ok(['darwin', 'linux', 'win32', 'freebsd'].includes(os.platform()));
assert.strictEqual(os.platform(), process.platform);
assert.ok(['BE', 'LE'].includes(os.endianness()));
assert.strictEqual(typeof os.type(), 'string');
assert.strictEqual(typeof os.arch(), 'string');
assert.strictEqual(os.devNull, '/dev/null');
assert.strictEqual(typeof os.constants.signals.SIGINT, 'number');
assert.strictEqual(os.constants.signals.SIGTERM, 15);
assert.strictEqual(typeof os.constants.errno.ENOENT, 'number');

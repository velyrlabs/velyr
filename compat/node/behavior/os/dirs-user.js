// Adapted from nodejs/node test/parallel/test-os.js (tmpdir, homedir, userInfo). Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const os = require('node:os');
const path = require('node:path');

assert.ok(path.isAbsolute(os.tmpdir()));
assert.ok(!os.tmpdir().endsWith('/') || os.tmpdir() === '/');
assert.ok(path.isAbsolute(os.homedir()));
const info = os.userInfo();
assert.strictEqual(typeof info.username, 'string');
assert.strictEqual(typeof info.uid, 'number');
assert.strictEqual(typeof os.networkInterfaces(), 'object');
assert.strictEqual(typeof os.machine(), 'string');
assert.strictEqual(typeof os.version(), 'string');

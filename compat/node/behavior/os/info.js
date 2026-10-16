// Adapted from nodejs/node test/parallel/test-os.js (info getters). Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const os = require('node:os');

assert.ok(os.totalmem() > 0);
assert.ok(os.freemem() > 0);
assert.ok(os.freemem() <= os.totalmem());
assert.ok(os.uptime() > 0);
assert.strictEqual(os.loadavg().length, 3);
const cpus = os.cpus();
assert.ok(Array.isArray(cpus) && cpus.length > 0);
assert.strictEqual(typeof cpus[0].model, 'string');
assert.strictEqual(typeof os.hostname(), 'string');
assert.strictEqual(typeof os.release(), 'string');
assert.ok(os.availableParallelism() >= 1);

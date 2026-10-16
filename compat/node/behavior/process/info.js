// Adapted from nodejs/node test/parallel/test-process-hrtime.js and test-process-versions.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');

assert.ok(Array.isArray(process.argv) && process.argv.length >= 2);
assert.strictEqual(typeof process.pid, 'number');
assert.strictEqual(typeof process.version, 'string');
assert.ok(process.version.startsWith('v'));
assert.strictEqual(typeof process.versions.node, 'string');
assert.strictEqual(typeof process.platform, 'string');
assert.strictEqual(typeof process.arch, 'string');
assert.strictEqual(process.cwd(), require('node:path').resolve('.'));
const t = process.hrtime();
assert.ok(Array.isArray(t) && t.length === 2);
assert.strictEqual(typeof process.hrtime.bigint(), 'bigint');
assert.ok(process.uptime() >= 0);
const mem = process.memoryUsage();
assert.ok(mem.rss > 0 && mem.heapUsed > 0);
assert.strictEqual(typeof process.release.name, 'string');
assert.ok(process.stdout && typeof process.stdout.write === 'function');
assert.strictEqual(typeof process.env, 'object');

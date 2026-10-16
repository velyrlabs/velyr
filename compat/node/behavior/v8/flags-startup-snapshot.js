// Adapted from nodejs/node test/parallel/test-v8-flags.js and test-startup-snapshot-api.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const v8 = require('node:v8');

assert.strictEqual(v8.setFlagsFromString('--stack-trace-limit=10'), undefined);
assert.throws(() => v8.setFlagsFromString(1), { code: 'ERR_INVALID_ARG_TYPE' });
assert.ok(!v8.startupSnapshot.isBuildingSnapshot());
assert.strictEqual(typeof v8.startupSnapshot.addDeserializeCallback, 'function');
assert.strictEqual(typeof v8.getHeapStatistics, 'function');
assert.strictEqual(typeof v8.serialize, 'function');
assert.strictEqual(typeof v8.deserialize, 'function');

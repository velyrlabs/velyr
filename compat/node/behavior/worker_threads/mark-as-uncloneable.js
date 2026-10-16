// Adapted from nodejs/node test/parallel/test-worker-message-port-transfer-fake-js-transferable.js (markAsUncloneable). Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { markAsUncloneable } = require('node:worker_threads');

assert.strictEqual(typeof markAsUncloneable, 'function');
const marked = { kept: 1 };
assert.strictEqual(markAsUncloneable(marked), undefined);
assert.deepStrictEqual(Object.keys(marked), ['kept']);

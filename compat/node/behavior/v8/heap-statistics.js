// Adapted from nodejs/node test/parallel/test-v8-stats.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const v8 = require('node:v8');

const stats = v8.getHeapStatistics();
const keys = [
  'total_heap_size', 'total_heap_size_executable', 'total_physical_size', 'total_available_size',
  'used_heap_size', 'heap_size_limit', 'malloced_memory', 'peak_malloced_memory',
  'does_zap_garbage', 'number_of_native_contexts', 'number_of_detached_contexts',
  'total_global_handles_size', 'used_global_handles_size', 'external_memory',
];
for (const key of keys) {
  assert.strictEqual(typeof stats[key], 'number', key);
  assert.ok(stats[key] >= 0, key);
}
assert.ok(stats.heap_size_limit > 0);
assert.ok(stats.used_heap_size <= stats.heap_size_limit);
assert.strictEqual(typeof v8.cachedDataVersionTag(), 'number');

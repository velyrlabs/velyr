// Adapted from nodejs/node test/parallel/test-process-next-tick.js and test-process-emit.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');

const order = [];
process.nextTick((a, b) => order.push(['tick', a, b]), 1, 2);
Promise.resolve().then(() => order.push('micro'));
process.nextTick(() => process.nextTick(() => order.push('nested')));
order.push('sync');
process.on('custom', (v) => order.push(['custom', v]));
assert.strictEqual(process.emit('custom', 9), true);
assert.strictEqual(process.listenerCount('custom'), 1);
process.on('exit', (code) => {
  assert.strictEqual(code, 0);
  assert.deepStrictEqual(order, [
    'sync', ['custom', 9], ['tick', 1, 2], 'nested', 'micro',
  ]);
});
assert.throws(() => process.nextTick('x'), { code: 'ERR_INVALID_ARG_TYPE' });

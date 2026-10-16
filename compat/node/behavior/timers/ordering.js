// Adapted from nodejs/node test/parallel/test-timers-ordering.js and test-timers-immediate-queue.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');

const order = [];
setTimeout(() => order.push('t10'), 10);
setTimeout(() => order.push('t1'), 1);
setImmediate(() => order.push('imm'));
process.nextTick(() => order.push('tick'));
Promise.resolve().then(() => order.push('micro'));
order.push('sync');
setTimeout(() => {
  assert.deepStrictEqual(order, ['sync', 'tick', 'micro', 'imm', 't1', 't10']);
}, 50);

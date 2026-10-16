// Adapted from nodejs/node test/parallel/test-event-emitter-order.js and test-event-emitter-listener-count.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const EventEmitter = require('node:events');

const e = new EventEmitter();
const seen = [];
e.on('x', (a, b) => seen.push(['first', a, b]));
e.prependListener('x', () => seen.push(['pre']));
e.once('x', () => seen.push(['once']));
assert.strictEqual(e.emit('x', 1, 2), true);
assert.strictEqual(e.emit('x', 3, 4), true);
assert.deepStrictEqual(seen, [['pre'], ['first', 1, 2], ['once'], ['pre'], ['first', 3, 4]]);
assert.strictEqual(e.emit('none'), false);
assert.strictEqual(e.listenerCount('x'), 2);
assert.deepStrictEqual(e.eventNames(), ['x']);

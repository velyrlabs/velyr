// Adapted from nodejs/node test/parallel/test-event-emitter-once.js and test-events-once.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const EventEmitter = require('node:events');

const e = new EventEmitter();
let calls = 0;
const fn = () => { calls++; };
e.on('a', fn);
e.removeListener('a', fn);
e.emit('a');
assert.strictEqual(calls, 0);
e.on('a', fn);
e.on('b', fn);
e.removeAllListeners('a');
assert.strictEqual(e.listenerCount('a'), 0);
assert.strictEqual(e.listenerCount('b'), 1);

const removed = [];
e.on('removeListener', (name) => removed.push(name));
e.off('b', fn);
assert.deepStrictEqual(removed, ['b']);

const p = EventEmitter.once(e, 'ready');
e.emit('ready', 1, 2);
p.then(common_check).then(() => process.stdout.write(''));
function common_check(args) {
  assert.deepStrictEqual(args, [1, 2]);
}

// Adapted from nodejs/node test/parallel/test-event-emitter-errors.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const EventEmitter = require('node:events');

const e = new EventEmitter();
const err = new Error('boom');
assert.throws(() => e.emit('error', err), err);
assert.throws(() => e.emit('error', 'str'), { code: 'ERR_UNHANDLED_ERROR' });
let got;
e.on('error', (x) => { got = x; });
e.emit('error', err);
assert.strictEqual(got, err);
const e2 = new EventEmitter({ captureRejections: false });
e2.setMaxListeners(1);
assert.strictEqual(e2.getMaxListeners(), 1);
assert.throws(() => e2.setMaxListeners(-1), { code: 'ERR_OUT_OF_RANGE' });
assert.strictEqual(EventEmitter.defaultMaxListeners, 10);

// Adapted from nodejs/node test/parallel/test-diagnostics-channel-tracing-channel-sync.js and -promise.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const dc = require('node:diagnostics_channel');

const tc = dc.tracingChannel('compat:trace');
assert.strictEqual(dc.channel('tracing:compat:trace:start'), tc.start);
assert.strictEqual(tc.hasSubscribers, false);
assert.strictEqual(tc.traceSync(() => 5), 5);

const log = [];
const handlers = {};
for (const event of ['start', 'end', 'asyncStart', 'asyncEnd', 'error']) {
  handlers[event] = (ctx) => log.push(`${event}:${ctx.error ? ctx.error.message : ctx.result}`);
}
tc.subscribe(handlers);
assert.strictEqual(tc.hasSubscribers, true);
assert.strictEqual(tc.traceSync(function (a) { return this.k + a; }, {}, { k: 1 }, 2), 3);
assert.deepStrictEqual(log, ['start:undefined', 'end:3']);
log.length = 0;
assert.throws(() => tc.traceSync(() => { throw new Error('boom'); }, {}), { message: 'boom' });
assert.deepStrictEqual(log, ['start:undefined', 'error:boom', 'end:boom']);
log.length = 0;

tc.tracePromise(async () => 'r', {}).then((value) => {
  assert.strictEqual(value, 'r');
  assert.deepStrictEqual(log, ['start:undefined', 'end:undefined', 'asyncStart:r', 'asyncEnd:r']);
  assert.strictEqual(tc.unsubscribe(handlers), true);
  assert.strictEqual(tc.hasSubscribers, false);
});

// Adapted from nodejs/node test/parallel/test-diagnostics-channel-bind-store.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const dc = require('node:diagnostics_channel');
const { AsyncLocalStorage } = require('node:async_hooks');

const als = new AsyncLocalStorage();
const ch = dc.channel('compat:bind-store');
assert.strictEqual(ch.hasSubscribers, false);
ch.bindStore(als, (data) => ({ wrapped: data }));
assert.strictEqual(ch.hasSubscribers, true);

let inSubscriber;
const onMessage = () => { inSubscriber = als.getStore(); };
ch.subscribe(onMessage);
const result = ch.runStores('d', function (a, b) {
  assert.deepStrictEqual(als.getStore(), { wrapped: 'd' });
  assert.strictEqual(this.tag, 'T');
  return a + b;
}, { tag: 'T' }, 1, 2);
assert.strictEqual(result, 3);
assert.deepStrictEqual(inSubscriber, { wrapped: 'd' });
assert.strictEqual(als.getStore(), undefined);

assert.strictEqual(ch.unbindStore(als), true);
assert.strictEqual(ch.unbindStore(als), false);
ch.runStores('y', () => assert.strictEqual(als.getStore(), undefined));
ch.unsubscribe(onMessage);
assert.strictEqual(ch.hasSubscribers, false);

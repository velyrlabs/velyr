// Adapted from nodejs/node test/parallel/test-diagnostics-channel-pub-sub.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const dc = require('node:diagnostics_channel');

const name = 'compat:pub-sub';
const ch = dc.channel(name);
assert.strictEqual(dc.channel(name), ch);
assert.ok(ch instanceof dc.Channel);
assert.strictEqual(ch.name, name);
assert.strictEqual(ch.hasSubscribers, false);
assert.strictEqual(dc.hasSubscribers(name), false);

const seen = [];
const onMessage = (message, channelName) => seen.push([message, channelName]);
dc.subscribe(name, onMessage);
assert.strictEqual(ch.hasSubscribers, true);
assert.strictEqual(dc.hasSubscribers(name), true);
ch.publish({ a: 1 });
assert.deepStrictEqual(seen, [[{ a: 1 }, name]]);
assert.strictEqual(dc.unsubscribe(name, onMessage), true);
assert.strictEqual(dc.unsubscribe(name, onMessage), false);
ch.publish('ignored');
assert.strictEqual(seen.length, 1);
assert.strictEqual(dc.hasSubscribers('compat:never-created'), false);
assert.throws(() => dc.subscribe(name, 'not a function'), { code: 'ERR_INVALID_ARG_TYPE' });
assert.throws(() => dc.channel(5), { code: 'ERR_INVALID_ARG_TYPE' });

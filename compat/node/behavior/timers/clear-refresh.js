// Adapted from nodejs/node test/parallel/test-timers-clear-object-does-not-throw-error.js and test-timers-refresh.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');

let fired = 0;
const cleared = setTimeout(() => { fired++; }, 5);
clearTimeout(cleared);
const imm = setImmediate(() => { fired++; });
clearImmediate(imm);
const iv = setInterval(() => {
  fired += 10;
  if (fired >= 30) clearInterval(iv);
}, 2);
const t = setTimeout(() => {}, 1000);
assert.strictEqual(typeof t.ref, 'function');
assert.strictEqual(typeof t.unref, 'function');
assert.strictEqual(t.hasRef(), true);
t.unref();
assert.strictEqual(t.hasRef(), false);
assert.strictEqual(t.refresh(), t);
assert.strictEqual(typeof t[Symbol.toPrimitive](), 'number');
clearTimeout(t);
clearTimeout(undefined);
clearTimeout(12345);
setTimeout(() => {
  assert.strictEqual(fired, 30);
}, 60);

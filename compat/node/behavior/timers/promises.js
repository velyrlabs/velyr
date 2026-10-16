// Adapted from nodejs/node test/parallel/test-timers-promises.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const timersPromises = require('node:timers/promises');

(async () => {
  assert.strictEqual(await timersPromises.setTimeout(5, 'value'), 'value');
  assert.strictEqual(await timersPromises.setImmediate('imm'), 'imm');
  const ac = new AbortController();
  const pending = timersPromises.setTimeout(1000, null, { signal: ac.signal });
  ac.abort();
  await assert.rejects(pending, { name: 'AbortError', code: 'ABORT_ERR' });
  const values = [];
  let n = 0;
  for await (const v of timersPromises.setInterval(2, 'tick')) {
    values.push(v);
    if (++n === 3) break;
  }
  assert.deepStrictEqual(values, ['tick', 'tick', 'tick']);
  assert.strictEqual(typeof timersPromises.scheduler.wait, 'function');
})().catch((err) => {
  console.error(err);
  process.exit(1);
});

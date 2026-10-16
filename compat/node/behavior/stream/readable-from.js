// Adapted from nodejs/node test/parallel/test-stream-readable-from.js and test-readable-from-iterator.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { Readable } = require('node:stream');

(async () => {
  const seen = [];
  for await (const v of Readable.from(['a', 'b', 'c'])) seen.push(v);
  assert.deepStrictEqual(seen, ['a', 'b', 'c']);

  const r = new Readable({ read() {} });
  r.push('x');
  r.push('y');
  r.push(null);
  r.setEncoding('utf8');
  let text = '';
  for await (const chunk of r) text += chunk;
  assert.strictEqual(text, 'xy');
  assert.strictEqual(r.readableEnded, true);

  const arr = await Readable.from([1, 2, 3, 4]).filter((n) => n % 2 === 0).map((n) => n * 10).toArray();
  assert.deepStrictEqual(arr, [20, 40]);
  const failing = new Readable({ read() { this.destroy(new Error('bad')); } });
  await assert.rejects(async () => { for await (const _ of failing); }, { message: 'bad' });
})().catch((err) => {
  console.error(err);
  process.exit(1);
});

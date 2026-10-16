// Adapted from nodejs/node test/parallel/test-stream-pipeline.js and test-stream-transform-objectmode-falsey-value.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { Readable, Transform, Writable, pipeline } = require('node:stream');
const { pipeline: pipelineP } = require('node:stream/promises');

const upper = () => new Transform({
  transform(chunk, enc, cb) { cb(null, chunk.toString().toUpperCase()); },
});

(async () => {
  const out = [];
  await pipelineP(
    Readable.from(['a', 'b']),
    upper(),
    new Writable({ write(chunk, enc, cb) { out.push(chunk.toString()); cb(); } }));
  assert.deepStrictEqual(out, ['A', 'B']);

  const err = await new Promise((resolve) => {
    pipeline(
      Readable.from(['x']),
      new Transform({ transform(c, e, cb) { cb(new Error('transform failed')); } }),
      new Writable({ write(c, e, cb) { cb(); } }),
      resolve);
  });
  assert.strictEqual(err.message, 'transform failed');
  const objs = [];
  await pipelineP(
    Readable.from([0, '', false, null].filter((v) => v !== null)),
    new Transform({ objectMode: true, transform(v, e, cb) { cb(null, v); } }),
    new Writable({ objectMode: true, write(v, e, cb) { objs.push(v); cb(); } }));
  assert.deepStrictEqual(objs, [0, '', false]);
})().catch((e) => {
  console.error(e);
  process.exit(1);
});

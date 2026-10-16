// Adapted from nodejs/node test/parallel/test-stream-writable-write-cb-error.js and test-stream-backpressure.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { Writable, PassThrough, finished } = require('node:stream');

const w = new Writable({
  highWaterMark: 4,
  write(chunk, enc, cb) { setImmediate(cb); },
});
assert.strictEqual(w.write('ab'), true);
assert.strictEqual(w.write('cdef'), false);
assert.strictEqual(w.writableNeedDrain, true);
w.once('drain', () => {
  assert.strictEqual(w.writableLength, 0);
  w.end();
});
finished(w, (err) => {
  assert.ifError(err);
  assert.strictEqual(w.writableFinished, true);
});

const pt = new PassThrough();
pt.end('data');
pt.on('data', (c) => assert.strictEqual(c.toString(), 'data'));
pt.on('end', () => assert.strictEqual(pt.readableEnded, true));
const bad = new Writable({ write(c, e, cb) { cb(new Error('write failed')); } });
bad.on('error', (err) => assert.strictEqual(err.message, 'write failed'));
bad.write('x');

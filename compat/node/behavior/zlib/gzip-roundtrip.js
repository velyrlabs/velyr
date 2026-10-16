// Adapted from nodejs/node test/parallel/test-zlib-from-string.js and test-zlib-sync-no-event.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const zlib = require('node:zlib');

const input = Buffer.from('hello hello hello hello hello');
const gz = zlib.gzipSync(input);
assert.strictEqual(gz[0], 0x1f);
assert.strictEqual(gz[1], 0x8b);
assert.ok(gz.length < input.length + 30);
assert.deepStrictEqual(zlib.gunzipSync(gz), input);
assert.deepStrictEqual(zlib.unzipSync(gz), input);
assert.throws(() => zlib.gunzipSync(Buffer.from('not gzip')), { code: 'Z_DATA_ERROR' });

zlib.gzip(input, (err, out) => {
  assert.ifError(err);
  zlib.gunzip(out, (err2, back) => {
    assert.ifError(err2);
    assert.deepStrictEqual(back, input);
  });
});

// Adapted from nodejs/node test/parallel/test-zlib-brotli.js and test-zlib.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const zlib = require('node:zlib');
const { pipeline, Readable } = require('node:stream');

const input = Buffer.from('stream me '.repeat(100));
assert.deepStrictEqual(zlib.brotliDecompressSync(zlib.brotliCompressSync(input)), input);

const chunks = [];
pipeline(
  Readable.from([input.subarray(0, 300), input.subarray(300)]),
  zlib.createGzip(),
  zlib.createGunzip(),
  async function (source) {
    for await (const chunk of source) chunks.push(chunk);
  },
  (err) => {
    assert.ifError(err);
    assert.deepStrictEqual(Buffer.concat(chunks), input);
  });

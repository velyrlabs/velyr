// Adapted from nodejs/node test/parallel/test-zlib-deflate-raw-inherits.js and test-zlib-params.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const zlib = require('node:zlib');

const input = Buffer.alloc(1000, 'a');
const deflated = zlib.deflateSync(input);
assert.strictEqual(deflated[0], 0x78);
assert.ok(deflated.length < 50);
assert.deepStrictEqual(zlib.inflateSync(deflated), input);
const raw = zlib.deflateRawSync(input, { level: 9 });
assert.deepStrictEqual(zlib.inflateRawSync(raw), input);
assert.ok(zlib.deflateSync(input, { level: 0 }).length > input.length);
assert.strictEqual(zlib.constants.Z_BEST_COMPRESSION, 9);
assert.throws(() => zlib.deflateSync(input, { level: 99 }), { code: 'ERR_OUT_OF_RANGE' });
assert.strictEqual(zlib.crc32('hello'), 0x3610a686);

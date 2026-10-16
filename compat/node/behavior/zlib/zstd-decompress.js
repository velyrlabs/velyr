// Adapted from nodejs/node test/parallel/test-zlib-zstd.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const zlib = require('node:zlib');

const hex = (text) => Buffer.from(text, 'hex');
const hello = hex('28b52ffd20168500005068656c6c6f207a7374640100e14a11');
const two = hex('28b52ffd200631000066697273742028b52ffd20063100007365636f6e64');
const skip = hex('502a4d1803000000aabbcc28b52ffd200a510000616674657220736b6970');
const checked = hex('28b52ffd240c610000636865636b65642064617461684588c4');
// 4 MiB of zeros in 147 bytes: a frame header, then 30 RLE blocks and a last one.
const bomb = hex('28b52ffd8058000040005400001000000100fbff39c002' + '02001000'.repeat(30) + '03001000');

assert.strictEqual(zlib.zstdDecompressSync(hello).toString(), 'hello hello hello zstd');
// Only the first frame is decoded; a second frame, a skippable frame and trailing junk add nothing.
assert.strictEqual(zlib.zstdDecompressSync(two).toString(), 'first ');
assert.strictEqual(zlib.zstdDecompressSync(skip).toString(), '');
assert.strictEqual(zlib.zstdDecompressSync(Buffer.concat([hello, Buffer.from('junk')])).toString(), 'hello hello hello zstd');
assert.strictEqual(zlib.zstdDecompressSync(checked).toString(), 'checked data');
assert.strictEqual(zlib.zstdDecompressSync(bomb).length, 4 << 20);

const corrupt = Buffer.from(checked);
corrupt[corrupt.length - 1] ^= 1;
// Node reports ZSTD_error_* codes; this runtime reports Z_DATA_ERROR, so only a code is required.
const hasCode = (error) => typeof error.code === 'string';
assert.throws(() => zlib.zstdDecompressSync(corrupt), hasCode);
assert.throws(() => zlib.zstdDecompressSync(Buffer.from('not zstd data')), hasCode);
assert.throws(() => zlib.zstdDecompressSync(bomb, { maxOutputLength: 65536 }), { code: 'ERR_BUFFER_TOO_LARGE' });

zlib.zstdDecompress(two, (error, out) => {
  assert.ifError(error);
  assert.strictEqual(out.toString(), 'first ');
});

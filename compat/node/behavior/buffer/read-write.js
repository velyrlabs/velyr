// Adapted from nodejs/node test/parallel/test-buffer-readint.js and test-buffer-write.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { Buffer } = require('node:buffer');

const b = Buffer.alloc(8);
b.writeUInt32BE(0xdeadbeef, 0);
b.writeInt16LE(-2, 4);
assert.strictEqual(b.readUInt32BE(0), 0xdeadbeef);
assert.strictEqual(b.readInt16LE(4), -2);
assert.strictEqual(b.readUInt8(0), 0xde);
b.writeDoubleLE(1.5, 0);
assert.strictEqual(b.readDoubleLE(0), 1.5);
b.writeBigUInt64BE(2n ** 63n, 0);
assert.strictEqual(b.readBigUInt64BE(0), 2n ** 63n);
assert.throws(() => b.readUInt32LE(6), { code: 'ERR_OUT_OF_RANGE' });
assert.throws(() => b.writeUInt8(256, 0), { code: 'ERR_OUT_OF_RANGE' });
assert.strictEqual(Buffer.from([1, 2, 3]).readUIntBE(0, 3), 0x010203);
assert.strictEqual(Buffer.alloc(4).write('abcdef', 1), 3);

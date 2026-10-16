// Adapted from nodejs/node test/parallel/test-string-decoder.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { StringDecoder } = require('node:string_decoder');

const d = new StringDecoder('utf8');
const euro = Buffer.from('€');
assert.strictEqual(d.write(euro.subarray(0, 1)), '');
assert.strictEqual(d.write(euro.subarray(1, 2)), '');
assert.strictEqual(d.write(euro.subarray(2)), '€');
assert.strictEqual(d.end(), '');
assert.strictEqual(d.write(Buffer.from([0xe2, 0x82])), '');
assert.strictEqual(d.end(), '�');
assert.strictEqual(d.encoding, 'utf8');
assert.strictEqual(new StringDecoder().write(Buffer.from('abc')), 'abc');
assert.strictEqual(d.write(Buffer.from([0xff])), '�');

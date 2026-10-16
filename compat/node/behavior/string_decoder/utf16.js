// Adapted from nodejs/node test/parallel/test-string-decoder.js (utf16le section). Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { StringDecoder } = require('node:string_decoder');

const d = new StringDecoder('utf16le');
const smile = Buffer.from('\u{1F600}', 'utf16le');
assert.strictEqual(smile.length, 4);
assert.strictEqual(d.write(smile.subarray(0, 1)), '');
assert.strictEqual(d.write(smile.subarray(1)), '\u{1F600}');
assert.strictEqual(d.write(Buffer.from('ab', 'utf16le')), 'ab');
assert.strictEqual(d.end(), '');
assert.strictEqual(d.encoding, 'utf16le');
assert.strictEqual(d.lastNeed, 0);

// Adapted from nodejs/node test/parallel/test-string-decoder.js (base64 and hex sections). Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const { StringDecoder } = require('node:string_decoder');

const b64 = new StringDecoder('base64');
assert.strictEqual(b64.write(Buffer.from('a')), '');
assert.strictEqual(b64.end(), 'YQ==');
const b64b = new StringDecoder('base64');
assert.strictEqual(b64b.write(Buffer.from('abc')), 'YWJj');
assert.strictEqual(b64b.write(Buffer.from('ab')), '');
assert.strictEqual(b64b.end(), 'YWI=');
const hex = new StringDecoder('hex');
assert.strictEqual(hex.write(Buffer.from([1, 0xab])), '01ab');
assert.strictEqual(hex.end(), '');
assert.strictEqual(new StringDecoder('latin1').write(Buffer.from([0xe9])), 'é');
assert.throws(() => new StringDecoder('nope'), { code: 'ERR_UNKNOWN_ENCODING' });

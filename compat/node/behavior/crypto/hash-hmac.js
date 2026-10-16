// Adapted from nodejs/node test/parallel/test-crypto-hash.js and test-crypto-hmac.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const crypto = require('node:crypto');

assert.strictEqual(
  crypto.createHash('sha256').update('abc').digest('hex'),
  'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
assert.strictEqual(
  crypto.createHash('md5').update('a').update('bc').digest('hex'),
  '900150983cd24fb0d6963f7d28e17f72');
assert.strictEqual(
  crypto.createHash('sha1').update('abc').digest('base64'),
  'qZk+NkcGgWq6PiVxeFDCbJzQ2J0=');
assert.strictEqual(
  crypto.createHmac('sha256', 'key').update('data').digest('hex'),
  '5031fe3d989c6d1537a013fa6e739da23463fdaec3b70137d828e36ace221bd0');
assert.strictEqual(crypto.createHash('sha512').update('').digest().length, 64);
assert.deepStrictEqual(crypto.getHashes().includes('sha256'), true);
assert.throws(() => crypto.createHash('nope'), /digest|Digest/);
const h = crypto.createHash('sha256');
h.digest();
assert.throws(() => h.update('x'), { code: 'ERR_CRYPTO_HASH_FINALIZED' });

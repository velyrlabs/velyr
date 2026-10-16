// Adapted from nodejs/node test/parallel/test-crypto-timing-safe-equal.js and test-webcrypto-digest.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const crypto = require('node:crypto');

assert.strictEqual(crypto.timingSafeEqual(Buffer.from('ab'), Buffer.from('ab')), true);
assert.strictEqual(crypto.timingSafeEqual(Buffer.from('ab'), Buffer.from('ac')), false);
assert.throws(() => crypto.timingSafeEqual(Buffer.from('a'), Buffer.from('ab')), { code: 'ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH' });
assert.strictEqual(typeof crypto.webcrypto.subtle.digest, 'function');
assert.strictEqual(globalThis.crypto.subtle, crypto.webcrypto.subtle);

crypto.webcrypto.subtle.digest('SHA-256', new TextEncoder().encode('abc')).then((digest) => {
  assert.strictEqual(
    Buffer.from(digest).toString('hex'),
    'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
}).catch((err) => {
  console.error(err);
  process.exit(1);
});

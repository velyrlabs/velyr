// Adapted from nodejs/node test/parallel/test-crypto-random.js and test-crypto-pbkdf2.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const crypto = require('node:crypto');

assert.strictEqual(crypto.randomBytes(16).length, 16);
assert.notStrictEqual(crypto.randomBytes(16).toString('hex'), crypto.randomBytes(16).toString('hex'));
assert.match(crypto.randomUUID(), /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
const n = crypto.randomInt(5, 10);
assert.ok(n >= 5 && n < 10);
assert.strictEqual(
  crypto.pbkdf2Sync('password', 'salt', 1, 20, 'sha1').toString('hex'),
  '0c60c80f961f0e71f3a9b524af6012062fe037a6');
assert.strictEqual(
  crypto.scryptSync('', '', 16, { N: 16, r: 1, p: 1 }).toString('hex'),
  '77d6576238657b203b19ca42c18a0497');
assert.strictEqual(crypto.getRandomValues(new Uint8Array(8)).length, 8);

crypto.randomBytes(8, (err, buf) => {
  assert.ifError(err);
  assert.strictEqual(buf.length, 8);
});

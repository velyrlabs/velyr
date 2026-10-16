// Adapted from nodejs/node test/parallel/test-v8-serdes.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const v8 = require('node:v8');

const value = {
  a: 1,
  list: [1, 'two', { when: new Date(5) }],
  map: new Map([[1, 'x']]),
  set: new Set([1, 2]),
  nothing: null,
  big: 10n,
  re: /ab+c/gi,
};
const bytes = v8.serialize(value);
assert.ok(Buffer.isBuffer(bytes));
const back = v8.deserialize(bytes);
assert.deepStrictEqual(back, value);
assert.deepStrictEqual(v8.deserialize(new Uint8Array(bytes)), value);
assert.throws(() => v8.serialize(() => {}), Error);
assert.throws(() => v8.deserialize('text'), { code: 'ERR_INVALID_ARG_TYPE' });
const circular = { name: 'c' };
circular.self = circular;
assert.strictEqual(v8.deserialize(v8.serialize(circular)).self.name, 'c');

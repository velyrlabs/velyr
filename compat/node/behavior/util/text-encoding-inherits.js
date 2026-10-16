// Adapted from nodejs/node test/parallel/test-util-inherits.js and test-whatwg-encoding-custom-textdecoder.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const util = require('node:util');

function A() {}
function B() {}
util.inherits(B, A);
assert.strictEqual(Object.getPrototypeOf(B.prototype), A.prototype);
assert.strictEqual(B.super_, A);

const enc = new util.TextEncoder();
assert.deepStrictEqual([...enc.encode('hé')], [0x68, 0xc3, 0xa9]);
assert.strictEqual(new util.TextDecoder().decode(new Uint8Array([0xe2, 0x82, 0xac])), '€');
assert.strictEqual(new util.TextDecoder('utf-8', { fatal: false }).decode(new Uint8Array([0xff])), '�');
assert.throws(() => new util.TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array([0xff])), TypeError);
const dep = util.deprecate(() => 7, 'old api', 'DEP_X');
assert.strictEqual(dep(), 7);
assert.strictEqual(util.stripVTControlCharacters('\u001b[31mred\u001b[0m'), 'red');

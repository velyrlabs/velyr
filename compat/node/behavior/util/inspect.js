// Adapted from nodejs/node test/parallel/test-util-inspect.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const util = require('node:util');

assert.strictEqual(util.inspect('a'), "'a'");
assert.strictEqual(util.inspect({ a: 1, b: [1, 2] }), '{ a: 1, b: [ 1, 2 ] }');
assert.strictEqual(util.inspect(new Map([[1, 2]])), 'Map(1) { 1 => 2 }');
assert.strictEqual(util.inspect(new Set([1])), 'Set(1) { 1 }');
assert.strictEqual(util.inspect([1, [2, [3, [4]]]], { depth: 1 }), '[ 1, [ 2, [Array] ] ]');
assert.strictEqual(util.inspect(Symbol('s')), 'Symbol(s)');
assert.strictEqual(util.inspect(10n), '10n');
assert.strictEqual(util.inspect(null), 'null');
assert.strictEqual(util.inspect(undefined), 'undefined');
const circular = {};
circular.self = circular;
assert.strictEqual(util.inspect(circular), '<ref *1> { self: [Circular *1] }');
class Foo { constructor() { this.x = 1; } }
assert.strictEqual(util.inspect(new Foo()), 'Foo { x: 1 }');
const custom = { [util.inspect.custom]() { return 'CUSTOM'; } };
assert.strictEqual(util.inspect(custom), 'CUSTOM');

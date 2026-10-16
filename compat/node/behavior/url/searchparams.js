// Adapted from nodejs/node test/parallel/test-whatwg-url-custom-searchparams.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');

const params = new URLSearchParams('a=1&b=2&a=3');
assert.deepStrictEqual(params.getAll('a'), ['1', '3']);
assert.strictEqual(params.get('b'), '2');
assert.strictEqual(params.has('c'), false);
params.append('c', 'x y');
assert.strictEqual(params.toString(), 'a=1&b=2&a=3&c=x+y');
params.delete('a');
params.sort();
assert.strictEqual(params.toString(), 'b=2&c=x+y');
assert.deepStrictEqual([...params.keys()], ['b', 'c']);
assert.strictEqual(new URLSearchParams({ q: 'é' }).toString(), 'q=%C3%A9');
const url = new URL('https://u:p@example.com:8080/p/a?x=1#h');
assert.strictEqual(url.searchParams.get('x'), '1');
url.searchParams.set('y', '2');
assert.strictEqual(url.search, '?x=1&y=2');

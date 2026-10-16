// Adapted from nodejs/node test/parallel/test-path-parse-format.js and test-path-relative.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const path = require('node:path');

assert.deepStrictEqual(path.parse('/home/u/file.txt'), {
  root: '/',
  dir: '/home/u',
  base: 'file.txt',
  ext: '.txt',
  name: 'file',
});
assert.strictEqual(path.format({ dir: '/x', name: 'f', ext: '.js' }), '/x/f.js');
assert.strictEqual(path.format({ root: '/', base: 'b' }), '/b');
assert.strictEqual(path.relative('/a/b/c', '/a/d'), '../../d');
assert.strictEqual(path.relative('/a', '/a'), '');
assert.strictEqual(path.resolve('/a/b', '../c'), '/a/c');
assert.strictEqual(path.resolve('/a', '/b'), '/b');
assert.strictEqual(path.sep, '/');
assert.strictEqual(path.delimiter, ':');

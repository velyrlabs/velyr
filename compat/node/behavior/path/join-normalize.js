// Adapted from nodejs/node test/parallel/test-path-join.js and test-path-normalize.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const path = require('node:path');

assert.strictEqual(path.join('/a', 'b', '../c'), '/a/c');
assert.strictEqual(path.join('a', '', 'b'), 'a/b');
assert.strictEqual(path.join(''), '.');
assert.strictEqual(path.normalize('/a//b/./c/..'), '/a/b');
assert.strictEqual(path.normalize('./a/../..'), '..');
assert.strictEqual(path.normalize('a/b/'), 'a/b/');
assert.strictEqual(path.win32.join('a', 'b'), 'a\\b');
assert.strictEqual(path.isAbsolute('/x'), true);
assert.strictEqual(path.isAbsolute('x'), false);
assert.throws(() => path.join(1), { code: 'ERR_INVALID_ARG_TYPE' });

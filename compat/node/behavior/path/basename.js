// Adapted from nodejs/node test/parallel/test-path-basename.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const path = require('node:path');

assert.strictEqual(path.basename('/a/b/c.txt'), 'c.txt');
assert.strictEqual(path.basename('/a/b/c.txt', '.txt'), 'c');
assert.strictEqual(path.basename('/a/b/'), 'b');
assert.strictEqual(path.basename(''), '');
assert.strictEqual(path.basename('/'), '');
assert.strictEqual(path.basename('a.txt', 'a.txt'), '');
assert.strictEqual(path.win32.basename('C:\\dir\\file.js'), 'file.js');
assert.strictEqual(path.posix.dirname('/a/b/c'), '/a/b');
assert.strictEqual(path.extname('x.tar.gz'), '.gz');
assert.strictEqual(path.extname('.bashrc'), '');

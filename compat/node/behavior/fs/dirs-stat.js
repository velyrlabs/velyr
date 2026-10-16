// Adapted from nodejs/node test/parallel/test-fs-readdir.js, test-fs-mkdir.js and test-fs-stat.js. Copyright Node.js contributors. MIT license.
// The runner passes a writable scratch directory in COMPAT_TMP.
'use strict';
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');

const dir = fs.mkdtempSync(path.join(process.env.COMPAT_TMP, 'dirs-'));
fs.mkdirSync(path.join(dir, 'x/y/z'), { recursive: true });
fs.writeFileSync(path.join(dir, 'x/f.txt'), '12345');
assert.deepStrictEqual(fs.readdirSync(path.join(dir, 'x')).sort(), ['f.txt', 'y']);
const entries = fs.readdirSync(path.join(dir, 'x'), { withFileTypes: true });
assert.strictEqual(entries.find((e) => e.name === 'y').isDirectory(), true);
assert.strictEqual(entries.find((e) => e.name === 'f.txt').isFile(), true);
const st = fs.statSync(path.join(dir, 'x/f.txt'));
assert.strictEqual(st.size, 5);
assert.strictEqual(st.isFile(), true);
assert.ok(st.mtimeMs > 0);
assert.throws(() => fs.mkdirSync(path.join(dir, 'x')), { code: 'EEXIST' });
assert.throws(() => fs.rmdirSync(path.join(dir, 'x')), { code: 'ENOTEMPTY' });
assert.throws(() => fs.statSync(path.join(dir, 'none')), { code: 'ENOENT' });
assert.strictEqual(fs.statSync(path.join(dir, 'none'), { throwIfNoEntry: false }), undefined);
fs.rmSync(dir, { recursive: true });
assert.strictEqual(fs.existsSync(dir), false);

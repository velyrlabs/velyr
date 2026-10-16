// Adapted from nodejs/node test/parallel/test-fs-write-file-sync.js and test-fs-readfile.js. Copyright Node.js contributors. MIT license.
// The runner passes a writable scratch directory in COMPAT_TMP.
'use strict';
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');

const dir = fs.mkdtempSync(path.join(process.env.COMPAT_TMP, 'rw-'));
const file = path.join(dir, 'a.txt');
fs.writeFileSync(file, 'hello');
fs.appendFileSync(file, ' world');
assert.strictEqual(fs.readFileSync(file, 'utf8'), 'hello world');
assert.deepStrictEqual(fs.readFileSync(file), Buffer.from('hello world'));
assert.throws(() => fs.readFileSync(path.join(dir, 'missing')), { code: 'ENOENT', syscall: 'open' });
assert.strictEqual(fs.existsSync(file), true);
fs.copyFileSync(file, path.join(dir, 'b.txt'));
fs.renameSync(path.join(dir, 'b.txt'), path.join(dir, 'c.txt'));
assert.strictEqual(fs.readFileSync(path.join(dir, 'c.txt'), 'utf8'), 'hello world');
fs.unlinkSync(path.join(dir, 'c.txt'));
assert.strictEqual(fs.existsSync(path.join(dir, 'c.txt')), false);

fs.readFile(file, 'utf8', (err, data) => {
  assert.ifError(err);
  assert.strictEqual(data, 'hello world');
  fs.rmSync(dir, { recursive: true });
});

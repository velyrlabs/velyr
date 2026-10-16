// Adapted from nodejs/node test/parallel/test-fs-promises.js and test-fs-write-stream.js. Copyright Node.js contributors. MIT license.
// The runner passes a writable scratch directory in COMPAT_TMP.
'use strict';
const assert = require('node:assert');
const fs = require('node:fs');
const fsp = require('node:fs/promises');
const path = require('node:path');

(async () => {
  const dir = await fsp.mkdtemp(path.join(process.env.COMPAT_TMP, 'p-'));
  const file = path.join(dir, 'a.txt');
  await fsp.writeFile(file, 'one');
  await fsp.appendFile(file, 'two');
  assert.strictEqual(await fsp.readFile(file, 'utf8'), 'onetwo');
  await assert.rejects(fsp.readFile(path.join(dir, 'nope')), { code: 'ENOENT' });
  assert.strictEqual((await fsp.stat(file)).size, 6);
  assert.deepStrictEqual(await fsp.readdir(dir), ['a.txt']);

  const out = path.join(dir, 'stream.txt');
  await new Promise((resolve, reject) => {
    const ws = fs.createWriteStream(out);
    ws.on('error', reject).on('finish', resolve);
    ws.write('abc');
    ws.end('def');
  });
  let text = '';
  for await (const chunk of fs.createReadStream(out, { encoding: 'utf8' })) text += chunk;
  assert.strictEqual(text, 'abcdef');
  const fh = await fsp.open(file, 'r');
  const { bytesRead, buffer } = await fh.read(Buffer.alloc(3), 0, 3, 0);
  assert.strictEqual(bytesRead, 3);
  assert.strictEqual(buffer.toString(), 'one');
  await fh.close();
  await fsp.rm(dir, { recursive: true });
})().catch((err) => {
  console.error(err);
  process.exit(1);
});

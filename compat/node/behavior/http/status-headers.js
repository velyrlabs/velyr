// Adapted from nodejs/node test/parallel/test-http-status-code.js and test-http-write-head.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const http = require('node:http');

assert.strictEqual(http.STATUS_CODES[404], 'Not Found');
assert.ok(http.METHODS.includes('GET') && http.METHODS.includes('PATCH'));
const server = http.createServer((req, res) => {
  if (req.url === '/missing') {
    res.statusCode = 404;
    res.setHeader('Set-Cookie', ['a=1', 'b=2']);
    assert.strictEqual(res.getHeader('set-cookie').length, 2);
    res.end();
  } else {
    assert.throws(() => res.setHeader('bad header', 'x'), { code: 'ERR_INVALID_HTTP_TOKEN' });
    res.writeHead(200);
    res.end();
    assert.strictEqual(res.headersSent, true);
  }
});
const get = (port, path) => new Promise((resolve, reject) => {
  http.get({ host: '127.0.0.1', port, path }, (res) => {
    res.resume();
    res.on('end', () => resolve(res));
  }).on('error', reject);
});
server.listen(0, '127.0.0.1', async () => {
  try {
    const { port } = server.address();
    const missing = await get(port, '/missing');
    assert.strictEqual(missing.statusCode, 404);
    assert.strictEqual(missing.statusMessage, 'Not Found');
    assert.deepStrictEqual(missing.headers['set-cookie'], ['a=1', 'b=2']);
    assert.strictEqual((await get(port, '/ok')).statusCode, 200);
    server.close();
  } catch (err) {
    console.error(err);
    process.exit(1);
  }
});

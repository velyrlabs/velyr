// Adapted from nodejs/node test/parallel/test-http-client-get-url.js and test-http-server.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const http = require('node:http');

const server = http.createServer((req, res) => {
  assert.strictEqual(req.method, 'GET');
  assert.strictEqual(req.url, '/hello?x=1');
  assert.strictEqual(req.headers['x-test'], 'yes');
  res.writeHead(201, { 'Content-Type': 'text/plain', 'X-Reply': 'ok' });
  res.end('world');
});
server.listen(0, '127.0.0.1', () => {
  const { port } = server.address();
  http.get({ host: '127.0.0.1', port, path: '/hello?x=1', headers: { 'X-Test': 'yes' } }, (res) => {
    assert.strictEqual(res.statusCode, 201);
    assert.strictEqual(res.headers['x-reply'], 'ok');
    assert.strictEqual(res.headers['content-type'], 'text/plain');
    let body = '';
    res.setEncoding('utf8');
    res.on('data', (c) => { body += c; });
    res.on('end', () => {
      assert.strictEqual(body, 'world');
      server.close();
    });
  }).on('error', (err) => { console.error(err); process.exit(1); });
});

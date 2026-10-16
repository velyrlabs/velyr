// Adapted from nodejs/node test/parallel/test-http-post-data.js and test-http-chunked.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const http = require('node:http');

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on('data', (c) => chunks.push(c));
  req.on('end', () => {
    assert.strictEqual(req.method, 'POST');
    const body = Buffer.concat(chunks).toString();
    res.setHeader('Content-Type', 'application/json');
    res.end(JSON.stringify({ echo: body, len: body.length }));
  });
});
server.listen(0, '127.0.0.1', () => {
  const { port } = server.address();
  const req = http.request({ host: '127.0.0.1', port, method: 'POST', path: '/' }, (res) => {
    assert.strictEqual(res.statusCode, 200);
    let body = '';
    res.on('data', (c) => { body += c; });
    res.on('end', () => {
      assert.deepStrictEqual(JSON.parse(body), { echo: 'part1part2', len: 10 });
      server.close();
    });
  });
  req.on('error', (err) => { console.error(err); process.exit(1); });
  req.write('part1');
  req.end('part2');
});

// Adapted from nodejs/node test/parallel/test-net-server-listen-options.js and test-net-connect-options.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const net = require('node:net');

const server = net.createServer((socket) => {
  socket.on('data', (d) => socket.write(d.toString().toUpperCase()));
  socket.on('end', () => socket.end());
});
server.listen(0, '127.0.0.1', () => {
  const { port, address, family } = server.address();
  assert.strictEqual(address, '127.0.0.1');
  assert.strictEqual(family, 'IPv4');
  const client = net.connect(port, '127.0.0.1', () => {
    assert.strictEqual(client.remoteAddress, '127.0.0.1');
    assert.strictEqual(client.remotePort, port);
    client.write('ping');
  });
  client.setEncoding('utf8');
  client.on('data', (d) => {
    assert.strictEqual(d, 'PING');
    client.end();
  });
  client.on('close', () => server.close());
  client.on('error', (err) => { console.error(err); process.exit(1); });
});

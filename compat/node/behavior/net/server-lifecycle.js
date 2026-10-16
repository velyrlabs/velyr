// Adapted from nodejs/node test/parallel/test-net-server-close.js and test-net-server-max-connections.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const net = require('node:net');

const events = [];
const server = net.createServer((socket) => {
  events.push('connection');
  socket.end('bye');
});
server.on('listening', () => events.push('listening'));
server.on('close', () => {
  events.push('close');
  assert.deepStrictEqual(events, ['listening', 'connection', 'close']);
  assert.strictEqual(server.listening, false);
});
server.listen(0, '127.0.0.1', () => {
  assert.strictEqual(server.listening, true);
  const client = net.connect(server.address().port, '127.0.0.1');
  let data = '';
  client.setEncoding('utf8');
  client.on('data', (d) => { data += d; });
  client.on('close', () => {
    assert.strictEqual(data, 'bye');
    server.close();
  });
  server.getConnections((err, count) => assert.ifError(err) || assert.strictEqual(typeof count, 'number'));
});

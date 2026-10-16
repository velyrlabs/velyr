// Adapted from nodejs/node test/parallel/test-net-isip.js and test-net-connect-refused.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const net = require('node:net');

assert.strictEqual(net.isIP('127.0.0.1'), 4);
assert.strictEqual(net.isIP('::1'), 6);
assert.strictEqual(net.isIP('example.com'), 0);
assert.strictEqual(net.isIPv4('1.2.3.4'), true);
assert.strictEqual(net.isIPv6('1.2.3.4'), false);
assert.throws(() => net.connect({ port: 70000 }), { code: 'ERR_SOCKET_BAD_PORT' });

// Bind a port, release it, then connect to the now closed port.
const probe = net.createServer();
probe.listen(0, '127.0.0.1', () => {
  const { port } = probe.address();
  probe.close(() => {
    const client = net.connect(port, '127.0.0.1');
    client.on('connect', () => { console.error('unexpected connect'); process.exit(1); });
    client.on('error', (err) => {
      assert.strictEqual(err.code, 'ECONNREFUSED');
      assert.strictEqual(err.syscall, 'connect');
    });
  });
});

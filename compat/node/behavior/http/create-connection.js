// Adapted from nodejs/node test/parallel/test-http-client-upgrade.js and test-http-agent-createconnection.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const http = require('node:http');
const net = require('node:net');

const server = http.createServer((req, res) => res.end(`${req.method} ${req.url}`));
server.on('upgrade', (req, socket) => {
  socket.write('HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: echo\r\n\r\n');
  socket.on('data', (data) => socket.write(data));
});

server.listen(0, '127.0.0.1', () => {
  const { port } = server.address();
  const used = [];
  const createConnection = (options) => {
    used.push(`${options.host}:${options.port}`);
    return net.connect(options);
  };
  // An address literal is dialed as it is: `lookup` is never asked (the runner only allows 127.0.0.1).
  let lookups = 0;
  const lookup = (hostname, options, callback) => {
    lookups += 1;
    callback(null, '127.0.0.1', 4);
  };

  http.get({ host: '127.0.0.1', port, path: '/a', createConnection }, (res) => {
    let body = '';
    res.setEncoding('utf8');
    res.on('data', (chunk) => { body += chunk; });
    res.on('end', () => {
      assert.strictEqual(body, 'GET /a');
      assert.deepStrictEqual(used, [`127.0.0.1:${port}`]);
      viaLookup();
    });
  });

  function viaLookup() {
    http.get({ host: '127.0.0.1', port, path: '/b', agent: false, lookup, family: 4 }, (res) => {
      res.resume();
      res.on('end', () => {
        assert.strictEqual(lookups, 0);
        upgrade();
      });
    });
  }

  function upgrade() {
    const req = http.request({ host: '127.0.0.1', port, createConnection, headers: { Connection: 'Upgrade', Upgrade: 'echo' } });
    req.on('upgrade', (res, socket) => {
      assert.strictEqual(res.statusCode, 101);
      socket.on('data', (data) => {
        assert.strictEqual(data.toString(), 'ping');
        socket.destroy();
        server.close();
      });
      socket.write('ping');
    });
    req.end();
  }
});

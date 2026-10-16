//! `node:http` client over a caller-supplied connection: `createConnection`, `lookup` and `family`.
//! The request is spoken by the JS client on a `node:net` socket, so the permission checks are
//! net's: the host the caller named and the address `lookup` answered are both authorized.
mod http_support;

use http_support::{Grant, check};

#[test]
fn create_connection_supplies_the_socket_and_the_response_is_parsed() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const net = req('net');
        const server = http.createServer(async (request, res) => {
          const body = await slurp(request);
          if (request.url === '/chunked') {
            res.write('ab');
            res.write('cd');
            return res.end('ef');
          }
          res.setHeader('X-Method', request.method);
          res.end(`${request.method}:${body}`);
        });
        const port = await listening(server);
        const seen = [];
        const createConnection = (options) => { seen.push([options.host, options.port]); return net.connect(options); };
        const base = { host: '127.0.0.1', port, createConnection };

        const got = await fetchOnce(http, { ...base, path: '/plain' });
        eq(got.res.statusCode, 200);
        eq(got.res.headers['x-method'], 'GET');
        eq(got.body, 'GET:');

        const posted = await fetchOnce(http, { ...base, method: 'POST', path: '/in' }, 'payload');
        eq(posted.body, 'POST:payload');

        const streamed = await new Promise((resolve, reject) => {
          const request = http.request({ ...base, method: 'PUT', path: '/in' }, async (res) => resolve((await slurp(res)).toString()));
          request.on('error', reject);
          request.write('one,');
          request.write('two');
          request.end();
        });
        eq(streamed, 'PUT:one,two');

        const chunked = await fetchOnce(http, { ...base, path: '/chunked' });
        eq(chunked.body, 'abcdef');
        eq(chunked.res.headers['transfer-encoding'], 'chunked');

        eq(seen.length, 4);
        eq(seen[0].join(':'), `127.0.0.1:${port}`);
        server.close();
        "#,
    );
}

#[test]
fn an_upgrade_hands_the_socket_and_the_leftover_bytes_to_the_caller() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const net = req('net');
        const server = http.createServer();
        server.on('upgrade', (request, socket) => {
          socket.write('HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: echo\r\n\r\nhello');
          socket.on('data', (data) => socket.write(data));
        });
        const port = await listening(server);
        const request = http.request({
          host: '127.0.0.1', port, createConnection: (options) => net.connect(options),
          headers: { Connection: 'Upgrade', Upgrade: 'echo' },
        });
        request.end();
        const [res, socket, head] = await once(request, 'upgrade');
        eq(res.statusCode, 101);
        eq(res.headers.upgrade, 'echo');
        // Bytes that arrived with the head are in `head`; the rest comes as 'data': no byte is lost.
        let got = head.toString();
        socket.on('data', (data) => { got += data; });
        while (got.length < 5) await sleep(5);
        eq(got, 'hello');
        socket.write('ping');
        while (got.length < 9) await sleep(5);
        eq(got, 'helloping');
        socket.destroy();
        server.close();
        "#,
    );
}

#[test]
fn lookup_and_family_choose_the_address_that_is_dialed() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const server = http.createServer((request, res) => res.end('via lookup'));
        const port = await listening(server);
        const calls = [];
        const lookup = (hostname, options, callback) => { calls.push([hostname, options.family]); callback(null, '127.0.0.1', 4); };
        const first = await fetchOnce(http, { host: 'localhost', port, lookup, family: 4, agent: false });
        eq(first.body, 'via lookup');
        eq(calls.length, 1);
        eq(calls[0][0], 'localhost');
        eq(calls[0][1], 4);
        const second = await fetchOnce(http, { host: 'localhost', port, family: 4, agent: false });
        eq(second.body, 'via lookup');
        server.close();
        "#,
    );
}

#[test]
fn a_custom_lookup_cannot_widen_what_net_connect_allows() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        let connections = 0;
        const server = http.createServer((request, res) => res.end('reached'));
        server.on('connection', () => { connections += 1; });
        const port = await listening(server);
        const calls = [];
        const toLoopback = (hostname, options, callback) => { calls.push(hostname); callback(null, '127.0.0.1', 4); };
        // The host the caller named is not allowed, whatever the lookup answers.
        const named = await new Promise((resolve) => http.get({ host: 'not-allowed.invalid', port, lookup: toLoopback, agent: false }).on('error', resolve));
        eq(named.code, 'ERR_ACCESS_DENIED');
        eq(calls.length, 0, 'the lookup must not run for a denied host');
        // The host is allowed but the address the lookup answered is not.
        const toElsewhere = (hostname, options, callback) => callback(null, '203.0.113.7', 4);
        const answered = await new Promise((resolve) => http.get({ host: 'localhost', port, lookup: toElsewhere, agent: false }).on('error', resolve));
        eq(answered.code, 'ERR_ACCESS_DENIED');
        eq(connections, 0);
        server.close();
        "#,
    );
}

#[test]
fn a_denied_create_connection_socket_and_unsupported_options_keep_failing_loudly() {
    check(
        Grant::Nothing,
        r#"
        const http = req('http');
        const net = req('net');
        const denied = await new Promise((resolve) => http.get({
          host: '127.0.0.1', port: 9, createConnection: (options) => net.connect(options),
        }).on('error', resolve));
        eq(denied.code, 'ERR_ACCESS_DENIED');
        eq((() => { try { http.get({ host: '127.0.0.1', port: 9, localAddress: '127.0.0.1' }); } catch (e) { return e.code; } })(), 'ERR_VELYR_UNSUPPORTED');
        "#,
    );
}

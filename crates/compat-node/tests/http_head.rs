//! The `node:http` request head and response headers cross the host boundary as value arrays and
//! `\0`-joined strings; these tests pin what the handler and the client still observe.
mod http_support;

use http_support::{Grant, check};

#[test]
fn request_head_fields_reach_the_handler() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const server = http.createServer((request, res) => {
          res.end(JSON.stringify({
            method: request.method,
            url: request.url,
            raw: request.rawHeaders.filter((_, i) => i % 2 === 0).filter((n) => /^x-/i.test(n)),
            dup: request.headers['x-dup'],
            remote: request.socket.remoteAddress,
          }));
        });
        const port = await listening(server);
        const { body } = await fetchOnce(http, {
          host: '127.0.0.1', port, method: 'PATCH', path: '/a%20b?q=%C3%BC',
          headers: { 'X-Dup': ['one', 'two'], 'x-Mixed': 'm' },
        });
        const seen = JSON.parse(body);
        eq(seen.method, 'PATCH');
        eq(seen.url, '/a%20b?q=%C3%BC');
        eq(seen.raw.join(','), 'x-dup,x-dup,x-mixed');
        eq(seen.dup, 'one, two');
        eq(seen.remote, '127.0.0.1');
        await new Promise((resolve) => server.close(resolve));
        "#,
    );
}

#[test]
fn header_value_with_nul_is_rejected() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        let code;
        const server = http.createServer((request, res) => {
          try {
            res.writeHead(200, { 'X-Bad': 'a\0b' });
            res.end('sent');
          } catch (error) {
            code = error.code;
            res.writeHead(200);
            res.end('rejected');
          }
        });
        const port = await listening(server);
        const { body } = await fetchOnce(http, { host: '127.0.0.1', port, path: '/' });
        eq(body, 'rejected');
        eq(code, 'ERR_INVALID_CHAR');
        await new Promise((resolve) => server.close(resolve));
        "#,
    );
}

#[test]
fn a_burst_of_requests_is_all_answered() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const server = http.createServer((request, res) => res.end(request.url));
        const port = await listening(server);
        const bodies = await Promise.all(Array.from({ length: 50 }, (_, i) =>
          fetchOnce(http, { host: '127.0.0.1', port, path: `/r${i}` }).then((r) => r.body)));
        bodies.forEach((body, i) => eq(body, `/r${i}`));
        await new Promise((resolve) => server.close(resolve));
        "#,
    );
}

#[test]
fn upgrade_refusal_path_still_responds() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const net = req('net');
        const server = http.createServer((request, res) => res.end('plain'));
        server.on('upgrade', (request, socket) => {
          socket.end('HTTP/1.1 400 Bad Request\r\nX-Refused: yes\r\nContent-Length: 0\r\n\r\n');
        });
        const port = await listening(server);
        const { res } = await fetchOnce(http, {
          host: '127.0.0.1', port, path: '/',
          headers: { Connection: 'Upgrade', Upgrade: 'x-test' },
        });
        eq(res.statusCode, 400);
        eq(res.headers['x-refused'], 'yes');
        await new Promise((resolve) => server.close(resolve));
        "#,
    );
}

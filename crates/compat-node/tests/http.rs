//! `node:http` server, client and Agent on V8 under the real event loop, over 127.0.0.1 and
//! ephemeral ports only. The HTTP protocol itself is io-http's; these tests cover the Node surface.
mod http_support;

use http_support::{Grant, check, run};

#[test]
fn a_server_answers_and_http_get_reads_the_response() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const server = http.createServer((request, res) => {
          res.setHeader('Content-Type', 'text/plain');
          res.setHeader('X-Seen', [request.method, request.url, request.headers.host.startsWith('127.0.0.1:')].join(' '));
          res.end('hello');
        });
        const port = await listening(server);
        const { res, body } = await new Promise((resolve, reject) => {
          http.get(`http://127.0.0.1:${port}/greet?x=1`, async (res) => resolve({ res, body: (await slurp(res)).toString() })).on('error', reject);
        });
        eq(res.statusCode, 200);
        eq(res.statusMessage, 'OK');
        eq(res.httpVersion, '1.1');
        eq(res.headers['content-type'], 'text/plain');
        eq(res.headers['content-length'], '5');
        eq(res.headers['x-seen'], 'GET /greet?x=1 true');
        eq(body, 'hello');
        eq(res.complete, true);
        await new Promise((resolve) => server.close(resolve));
        eq(server.listening, false);
        eq(server.address(), null);
        "#,
    );
}

#[test]
fn streaming_a_large_body_stops_the_writer_until_the_peer_reads() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const CHUNK = 64 * 1024;
        const TOTAL = 24 * 1024 * 1024;
        let sawFalse = false;
        let written = 0;
        const server = http.createServer(async (request, res) => {
          res.writeHead(200, { 'Content-Type': 'application/octet-stream' });
          const chunk = Buffer.alloc(CHUNK, 7);
          while (written < TOTAL) {
            written += CHUNK;
            if (!res.write(chunk)) {
              sawFalse = true;
              await once(res, 'drain');
            }
          }
          res.end();
        });
        const port = await listening(server);
        const received = await new Promise((resolve, reject) => {
          http.get({ port, host: '127.0.0.1' }, (res) => {
            res.pause();
            // The peer is slow: a bounded pipeline must stop the writer long before TOTAL.
            sleep(400).then(() => {
              eq(sawFalse, true, 'write() never returned false');
              if (!(written < TOTAL)) throw new Error('the writer ran to completion against a paused reader: ' + written);
              let size = 0;
              res.on('data', (part) => { size += part.length; });
              res.on('end', () => resolve(size));
              res.resume();
            }).catch(reject);
          }).on('error', reject);
        });
        eq(received, TOTAL);
        server.close();
        "#,
    );
}

#[test]
fn request_bodies_stream_to_the_handler_and_back() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const server = http.createServer((request, res) => {
          res.setHeader('X-Method', request.method);
          request.pipe(res);
        });
        const port = await listening(server);
        const pieces = Array.from({ length: 64 }, (_, i) => Buffer.alloc(16 * 1024, i));
        const { res, body } = await new Promise((resolve, reject) => {
          const request = http.request({ port, host: '127.0.0.1', method: 'POST', path: '/echo' }, async (res) => {
            resolve({ res, body: await slurp(res) });
          });
          request.on('error', reject);
          (async () => {
            for (const piece of pieces) {
              if (!request.write(piece)) await once(request, 'drain');
            }
            request.end();
          })();
        });
        eq(res.headers['x-method'], 'POST');
        eq(res.headers['transfer-encoding'], 'chunked');
        eq(Buffer.compare(body, Buffer.concat(pieces)), 0);
        server.close();
        "#,
    );
}

#[test]
fn a_small_request_body_carries_its_length() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        let seenLength;
        const server = http.createServer(async (request, res) => {
          seenLength = request.headers['content-length'];
          res.end(await slurp(request));
        });
        const port = await listening(server);
        const small = await new Promise((resolve, reject) => {
          const request = http.request({ port, host: '127.0.0.1', method: 'POST' }, async (res) => resolve(res.headers['content-length'] + ':' + (await slurp(res))));
          request.on('error', reject);
          request.end('exact');
        });
        eq(seenLength, '5');
        eq(small, '5:exact');
        server.close();
        "#,
    );
}

#[test]
fn heads_statuses_and_helpers_follow_node() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const server = http.createServer((request, res) => {
          res.appendHeader('Set-Cookie', 'a=1');
          res.appendHeader('Set-Cookie', 'b=2');
          res.writeHead(404, 'Not Here', ['X-One', '1']);
          eq(res.headersSent, true);
          const late = (() => { try { res.setHeader('X', 'y'); } catch (e) { return e.code; } })();
          res.end(late);
        });
        const port = await listening(server);
        const miss = await fetchOnce(http, { port, host: '127.0.0.1', path: '/missing' });
        eq(miss.res.statusCode, 404);
        const same = (a, b) => eq(JSON.stringify(a), JSON.stringify(b));
        same(miss.res.headers['set-cookie'], ['a=1', 'b=2']);
        eq(miss.res.headers['x-one'], '1');
        eq(miss.body, 'ERR_HTTP_HEADERS_SENT');
        server.close();
        "#,
    );
}

#[test]
fn bodiless_responses_end_without_a_body() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const server = http.createServer((request, res) => {
          if (request.url === '/empty') {
            res.statusCode = 204;
            res.end();
          } else {
            res.setHeader('Content-Length', '9');
            res.end(request.method === 'HEAD' ? undefined : 'headhead!');
          }
        });
        const port = await listening(server);
        const empty = await fetchOnce(http, { port, host: '127.0.0.1', path: '/empty' });
        eq(empty.res.statusCode, 204);
        eq(empty.body, '');
        const head = await fetchOnce(http, { port, host: '127.0.0.1', path: '/x', method: 'HEAD' });
        eq(head.res.statusCode, 200);
        eq(head.body, '');
        server.close();
        "#,
    );
}

#[test]
fn static_tables_and_validators_follow_node() {
    check(
        Grant::Nothing,
        r#"
        const http = req('http');
        eq(http.STATUS_CODES[418], "I'm a Teapot");
        eq(http.METHODS.includes('PATCH'), true);
        eq((() => { try { http.validateHeaderName('bad name'); } catch (e) { return e.code; } })(), 'ERR_INVALID_HTTP_TOKEN');
        eq((() => { try { http.validateHeaderValue('x', 'a\nb'); } catch (e) { return e.code; } })(), 'ERR_INVALID_CHAR');
        eq((() => { try { new http.ServerResponse({}).writeHead(99); } catch (e) { return e.code; } })(), 'ERR_HTTP_INVALID_STATUS_CODE');
        eq((() => { try { new http.ServerResponse({}).addTrailers({}); } catch (e) { return e.code; } })(), 'ERR_VELYR_UNSUPPORTED');
        "#,
    );
}

#[test]
fn errors_name_the_node_codes() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        const probe = http.createServer();
        const port = await listening(probe);
        await new Promise((resolve) => probe.close(resolve));
        const refused = await new Promise((resolve) => http.get({ port, host: '127.0.0.1' }).on('error', resolve));
        eq(refused.code, 'ECONNREFUSED');
        eq(refused.syscall, 'connect');
        eq(refused.port, port);
        const busy = http.createServer();
        const busyPort = await listening(busy);
        const again = http.createServer();
        again.listen(busyPort, '127.0.0.1');
        eq((await once(again, 'error'))[0].code, 'EADDRINUSE');
        busy.close();
        eq((() => { try { http.request({ port, host: '127.0.0.1', path: '/a b' }); } catch (e) { return e.code; } })(), 'ERR_UNESCAPED_CHARACTERS');
        eq((() => { try { http.request('https://127.0.0.1/'); } catch (e) { return e.code; } })(), 'ERR_INVALID_PROTOCOL');
        eq((() => { try { http.get({ socketPath: '/tmp/x' }); } catch (e) { return e.code; } })(), 'ERR_VELYR_UNSUPPORTED');
        eq((() => { try { http.createServer().listen('/tmp/pipe'); } catch (e) { return e.code; } })(), 'ERR_VELYR_UNSUPPORTED');
        "#,
    );
}

#[test]
fn listen_and_request_without_permission_are_access_denied() {
    check(
        Grant::Nothing,
        r#"
        const http = req('http');
        const server = http.createServer();
        server.listen(0, '127.0.0.1');
        eq((await once(server, 'error').catch((e) => [e]))[0].code, 'ERR_ACCESS_DENIED');
        const denied = await new Promise((resolve) => http.get({ port: 9, host: '127.0.0.1' }).on('error', resolve));
        eq(denied.code, 'ERR_ACCESS_DENIED');
        "#,
    );
}

#[test]
fn an_agent_limits_concurrent_requests_and_reuses_connections() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        let running = 0;
        let peak = 0;
        const server = http.createServer(async (request, res) => {
          running += 1;
          peak = Math.max(peak, running);
          await sleep(40);
          running -= 1;
          res.end('ok');
        });
        const port = await listening(server);
        const agent = new http.Agent({ keepAlive: true, maxSockets: 2 });
        eq(agent.keepAlive, true);
        const all = await Promise.all(Array.from({ length: 6 }, () => fetchOnce(http, { port, host: '127.0.0.1', agent })));
        eq(all.every(({ body }) => body === 'ok'), true);
        eq(peak <= 2, true, 'peak ' + peak);
        eq(peak, 2);
        eq(Object.keys(agent.sockets).length, 0);
        agent.destroy();
        const closing = await fetchOnce(http, { port, host: '127.0.0.1', agent: false });
        eq(closing.res.headers.connection, 'close');
        server.close();
        "#,
    );
}

#[test]
fn a_client_that_leaves_aborts_the_request_on_both_sides() {
    check(
        Grant::Loopback,
        r#"
        const http = req('http');
        let seen;
        const gone = new Promise((resolve) => { seen = resolve; });
        const server = http.createServer((request, res) => {
          request.on('aborted', () => seen('aborted'));
          res.on('close', () => seen('closed'));
        });
        const port = await listening(server);
        const request = http.get({ port, host: '127.0.0.1' });
        const hangup = once(request, 'error');
        await sleep(100);
        request.destroy();
        eq((await hangup)[0].code, 'ECONNRESET');
        eq(['aborted', 'closed'].includes(await gone), true);
        server.close();
        "#,
    );
}

#[test]
fn http2_loads_and_refuses_to_open_sessions() {
    check(
        Grant::Nothing,
        r#"
        const http2 = req('http2');
        eq(http2.constants.HTTP2_HEADER_STATUS, ':status');
        eq((() => { try { http2.createServer(); } catch (e) { return e.code; } })(), 'ERR_VELYR_UNSUPPORTED');
        eq((() => { try { http2.connect('http://x'); } catch (e) { return e.code; } })(), 'ERR_VELYR_UNSUPPORTED');
        "#,
    );
}

#[test]
fn a_failing_script_is_reported_not_swallowed() {
    assert!(run(Grant::Nothing, "throw new Error('boom');").contains("boom"));
}

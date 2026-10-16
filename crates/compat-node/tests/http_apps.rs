//! Real-world shapes on `node:http` / `node:https`: an Express-style app, an `ws`-style upgrade and a
//! TLS server with a generated certificate. The fixtures are simplified look-alikes, see their headers.
mod http_support;

use http_support::{Grant, check, fixture_prelude, self_signed};

const EXPRESS: &str = include_str!("fixtures/http/express.js");
const WS: &str = include_str!("fixtures/http/ws.js");

#[test]
fn an_express_app_routes_params_queries_json_and_errors() {
    let prelude = fixture_prelude(&[("express", EXPRESS)]);
    check(
        Grant::Loopback,
        &format!(
            r#"{prelude}
        const http = req('http');
        const express = fixture('express');
        const app = express();
        app.use((request, res, next) => {{ res.set('X-App', 'mini'); next(); }});
        app.get('/users/:id', (request, res) => res.json({{ id: request.params.id, q: request.query }}));
        app.post('/echo', express.json(), (request, res) => res.status(201).send(request.body));
        app.get('/boom', () => {{ throw new Error('kaput'); }});
        app.get('/text', (request, res) => res.send('plain'));
        app.use((error, request, res, next) => res.status(500).send('handled: ' + error.message));
        const server = app.listen(0, '127.0.0.1');
        await once(server, 'listening');
        const port = server.address().port;
        const get = (path) => fetchOnce(http, {{ port, host: '127.0.0.1', path }});

        const user = await get('/users/42?x=1&y=two');
        eq(user.res.statusCode, 200);
        eq(user.res.headers['x-app'], 'mini');
        eq(user.res.headers['content-type'], 'application/json; charset=utf-8');
        eq(user.body, '{{"id":"42","q":{{"x":"1","y":"two"}}}}');

        const posted = await fetchOnce(http, {{ port, host: '127.0.0.1', path: '/echo', method: 'POST', headers: {{ 'Content-Type': 'application/json' }} }}, '{{"a":[1,2]}}');
        eq(posted.res.statusCode, 201);
        eq(posted.body, '{{"a":[1,2]}}');

        eq((await get('/text')).body, 'plain');
        eq((await get('/boom')).body, 'handled: kaput');
        const missing = await get('/nope');
        eq(missing.res.statusCode, 404);
        eq(missing.body, 'Cannot GET /nope');
        server.close();
        "#
        ),
    );
}

#[test]
fn a_ws_server_upgrades_and_exchanges_messages_over_the_http_server() {
    let prelude = fixture_prelude(&[("ws", WS)]);
    check(
        Grant::Loopback,
        &format!(
            r#"{prelude}
        const http = req('http');
        const WebSocket = fixture('ws');
        const server = http.createServer((request, res) => res.end('not a socket'));
        const wss = new WebSocket.WebSocketServer({{ server }});
        wss.on('connection', (ws, request) => {{
          ws.on('message', (data, isBinary) => ws.send(isBinary ? Buffer.concat([Buffer.from('bin:'), data]) : 'echo:' + data));
        }});
        const port = await listening(server);

        const client = new WebSocket(`ws://127.0.0.1:${{port}}/chat`);
        await once(client, 'open');
        const big = 'x'.repeat(70000);
        const received = [];
        client.on('message', (data, isBinary) => received.push([isBinary, isBinary ? data.toString() : data]));
        client.send('hello');
        client.send(big);
        client.send(Buffer.from([1, 2, 3]));
        const pong = once(client, 'pong');
        client.ping(Buffer.from('p'));
        eq((await pong)[0].toString(), 'p');
        while (received.length < 3) await sleep(10);
        eq(received[0][1], 'echo:hello');
        eq(received[1][1], 'echo:' + big);
        eq(received[2][0], true);
        eq(received[2][1], 'bin:\u0001\u0002\u0003');
        const closed = once(client, 'close');
        client.close(1000);
        eq((await closed)[0], 1000);

        // A plain request to the same server still reaches the request listener.
        eq((await fetchOnce(http, {{ port, host: '127.0.0.1' }})).body, 'not a socket');

        // A handshake that ws refuses is answered as an ordinary HTTP response.
        const refused = await fetchOnce(http, {{ port, host: '127.0.0.1', headers: {{ Connection: 'Upgrade', Upgrade: 'websocket' }} }}).catch((e) => e);
        eq(refused.res?.statusCode, 400);
        eq(refused.body, 'Bad Request');
        wss.close();
        server.close();
        "#
        ),
    );
}

#[test]
fn an_https_server_with_a_generated_certificate_serves_a_client_that_trusts_it() {
    let (cert, key) = self_signed();
    let cert = serde_json::to_string(&cert).unwrap();
    let key = serde_json::to_string(&key).unwrap();
    check(
        Grant::Loopback,
        &format!(
            r#"
        const https = req('https');
        const cert = {cert};
        const key = {key};
        const server = https.createServer({{ cert, key }}, (request, res) => {{
          res.setHeader('X-Secure', String(request.socket.encrypted === true));
          request.pipe(res);
        }});
        const port = await listening(server);
        const got = await fetchOnce(https, {{ host: 'localhost', port, ca: cert, method: 'POST', agent: false }}, 'over tls');
        eq(got.res.statusCode, 200);
        eq(got.res.headers['x-secure'], 'true');
        eq(got.body, 'over tls');

        // Without the certificate in `ca` the handshake is refused.
        const untrusted = await new Promise((resolve) => https.get({{ host: 'localhost', port, agent: false }}).on('error', resolve));
        eq(typeof untrusted.code, 'string');

        const insecure = (() => {{ try {{ https.get({{ host: 'localhost', port, rejectUnauthorized: false }}); }} catch (e) {{ return e.code; }} }})();
        eq(insecure, 'ERR_VELYR_UNSUPPORTED');
        server.close();
        "#
        ),
    );
}

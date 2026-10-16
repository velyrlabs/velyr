import http from 'node:http';
import { request } from 'undici';

const server = http.createServer((req, res) => {
  res.setHeader('X-Seen', req.headers['x-test'] ?? '');
  res.end(`hello ${req.method}`);
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
try {
  const { statusCode, headers, body } = await request(`http://127.0.0.1:${server.address().port}/`, {
    headers: { 'x-test': 'yes' },
  });
  const text = await body.text();
  if (statusCode !== 200 || text !== 'hello GET' || headers['x-seen'] !== 'yes') throw new Error(`${statusCode} ${text}`);
  console.log('ok');
} finally {
  server.close();
}

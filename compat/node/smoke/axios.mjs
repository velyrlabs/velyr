import http from 'node:http';
import axios from 'axios';

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on('data', (c) => chunks.push(c));
  req.on('end', () => {
    res.setHeader('Content-Type', 'application/json');
    res.end(JSON.stringify({ method: req.method, body: Buffer.concat(chunks).toString() }));
  });
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;
try {
  const got = await axios.get(`${base}/x`, { params: { a: 1 } });
  if (got.status !== 200 || got.data.method !== 'GET') throw new Error(JSON.stringify(got.data));
  const posted = await axios.post(`${base}/x`, { n: 1 });
  if (posted.data.body !== '{"n":1}') throw new Error(JSON.stringify(posted.data));
  console.log('ok');
} finally {
  server.close();
}

import express from 'express';

const app = express();
app.get('/hi', (req, res) => res.json({ ok: true, q: req.query.x }));
const server = app.listen(0, '127.0.0.1', async () => {
  try {
    const { port } = server.address();
    const res = await fetch(`http://127.0.0.1:${port}/hi?x=1`);
    const body = await res.json();
    if (res.status !== 200 || body.ok !== true || body.q !== '1') throw new Error(JSON.stringify(body));
    server.close(() => console.log('ok'));
  } catch (err) {
    console.error(err);
    process.exit(1);
  }
});
